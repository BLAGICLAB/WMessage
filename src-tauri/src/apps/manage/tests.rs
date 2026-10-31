use super::*;

fn temp_db() -> (tempfile::TempDir, Connection) {
    let tmp = tempfile::tempdir().unwrap();
    let conn = Connection::open(tmp.path().join("test.db")).unwrap();
    conn.execute(db_apps::APPS_DDL, []).unwrap();
    conn.execute(db_apps::APP_VERSIONS_DDL, []).unwrap();
    // app_runs 一并建：三张表在生产里由 db::ensure 一次建齐，缺一张的夹具
    // 会让「删干净三张表」这类用例撞上 no such table 而不是它要验的东西
    conn.execute(db_apps::APP_RUNS_DDL, []).unwrap();
    (tmp, conn)
}

const SCRIPT: &str = "import sys\nprint('##RESULT## {\"summary\":\"hi\"}')\n";

/// 直接落一行已上架程序（归档/恢复测试只需要库里的行，不碰文件集）
fn seed_enabled(conn: &Connection, id: &str) -> App {
    let app = App {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        icon: "📦".into(),
        input_schema: r#"{"kind":"folder"}"#.into(),
        script_path: "app.py".into(),
        manual: String::new(),
        status: mc::STATUS_ENABLED.into(),
        enabled_at: Some(1000),
        created_at: 1000,
        updated_at: 1000,
        timeout_secs: None,
    };
    db_apps::upsert_app(conn, &app).unwrap();
    app
}

fn create_args(
    id: &str,
) -> (
    String,
    String,
    String,
    String,
    String,
    String,
    Vec<(String, String)>,
    String,
) {
    (
        id.to_string(),
        "工资单匹配".to_string(),
        "把流水和工资表对上".to_string(),
        "💰".to_string(),
        r#"{"kind":"folder"}"#.to_string(),
        SCRIPT.to_string(),
        Vec::new(),
        String::new(),
    )
}

#[test]
fn create_writes_draft_with_script_and_meta() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    let app = create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    assert_eq!(app.status, "draft");
    assert_eq!(app.script_path, "app.py");
    assert!(app.enabled_at.is_none());
    assert!(dir.join("app.py").is_file());
    assert!(db_apps::get_app(&conn, "wage").unwrap().is_some());
}

#[test]
fn app_id_rejects_non_component_values() {
    // `apps_root.join(id)` 必须恰好落在 `apps/<id>/`：单个 "." 会把目录折叠回
    // apps 根，归档后的彻底删除就成了对整个 apps 根的 remove_dir_all
    for bad in ["", ".", "..", "a/b", "a\\b", "a b", "中", "a:b"] {
        assert!(
            validate_app_id(bad).is_err(),
            "id {bad:?} 不该被当成合法程序 id"
        );
    }
    for ok in ["wage", "pdf-merge-2", "hello_world"] {
        assert!(validate_app_id(ok).is_ok(), "id {ok:?} 该被放行");
    }
}

#[test]
fn create_rejects_dot_id_without_touching_root() {
    let (tmp, conn) = temp_db();
    let (id, name, desc, icon, schema, script, files, manual) = create_args(".");
    let err = create_app_core(
        tmp.path(),
        &conn,
        &id,
        &name,
        &desc,
        &icon,
        &schema,
        &script,
        &files,
        &manual,
        None,
        1,
    )
    .unwrap_err();
    assert!(matches!(err, CommandError::InvalidArgument { .. }));
    assert!(
        db_apps::get_app(&conn, ".").unwrap().is_none(),
        "拒绝后不该留下折叠到根的库行"
    );
}

#[test]
fn create_rejects_duplicate_id_without_overwrite() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    let err = create_app_core(
        &dir,
        &conn,
        &id,
        &name,
        &desc,
        &icon,
        &schema,
        "print('x')",
        &files,
        &manual,
        None,
        2,
    )
    .unwrap_err();
    assert!(err.to_string().contains("已存在"), "{err}");
    // 原脚本未被覆盖
    let kept = std::fs::read_to_string(dir.join("app.py")).unwrap();
    assert_eq!(kept, SCRIPT);
}

#[test]
fn create_rejects_bad_id_and_meta_and_schema() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("x");
    let (_i, name, desc, icon, schema, script, files, manual) = create_args("ok");
    // id 含 ..
    let err = create_app_core(
        &dir, &conn, "../evil", &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap_err();
    assert!(err.to_string().contains("id"), "{err}");
    // 名称超长
    let long_name = "名".repeat(mc::NAME_MAX + 1);
    let err = create_app_core(
        &dir, &conn, "ok", &long_name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap_err();
    assert!(err.to_string().contains("名称"), "{err}");
    // inputSchema 表单字段 type 非法
    let err = create_app_core(
        &dir,
        &conn,
        "ok",
        &name,
        &desc,
        &icon,
        r#"{"kind":"form","fields":[{"key":"a","label":"A","type":"blob"}]}"#,
        &script,
        &files,
        &manual,
        None,
        1,
    )
    .unwrap_err();
    assert!(err.to_string().contains("text/number/date"), "{err}");
    // 空脚本
    let err = create_app_core(
        &dir, &conn, "ok", &name, &desc, &icon, &schema, "  ", &files, &manual, None, 1,
    )
    .unwrap_err();
    assert!(err.to_string().contains("script"), "{err}");
    assert!(!dir.exists(), "全部拒绝后不得留下目录");
}

/// 库是真相源：文件集写失败（data/ 被同名文件占位）时上抛错误、不落库行，
/// 预置文件原样保留（不回滚不删除，重试即整体覆盖）
#[test]
fn create_fs_failure_leaves_no_db_row_and_spares_existing_data() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("w");
    let (id, name, desc, icon, schema, script, _files, manual) = create_args("w");
    // 预置冲突：dir/data 是文件 → 附加文件 data/x.csv 的父目录创建必失败
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("data"), "占位文件").unwrap();
    let files = vec![("data/x.csv".to_string(), "a,b".to_string())];
    let err = create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("File exists") || err.to_string().contains("失败"),
        "{err}"
    );
    assert!(
        db_apps::get_app(&conn, "w").unwrap().is_none(),
        "fs 失败不得落库行"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("data")).unwrap(),
        "占位文件"
    );
}

#[test]
fn update_rewrites_script_and_flips_status_to_enabled() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    let next = update_app_core(
        &dir,
        &conn,
        "wage",
        Some("print('v2')"),
        &[],
        None,
        None,
        None,
        None,
        None,
        Some("enabled"),
        None,
        2,
    )
    .unwrap();
    assert_eq!(next.status, "enabled");
    assert_eq!(next.enabled_at, Some(2));
    assert_eq!(
        std::fs::read_to_string(dir.join("app.py")).unwrap(),
        "print('v2')"
    );
    // created_at 保留
    assert_eq!(next.created_at, 1);
}

#[test]
fn archive_hides_app_from_program_page_and_unarchive_brings_it_back_as_draft() {
    let (_tmp, conn) = temp_db();
    let app = seed_enabled(&conn, "salary");
    // 归档：程序页列非归档，归档后不该在里面
    let archived = archive_core(&conn, &app.id, 2000).unwrap();
    assert_eq!(archived.status, mc::STATUS_ARCHIVED);
    let active_ids: Vec<String> = db_apps::list_active_apps(&conn)
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert!(
        !active_ids.contains(&app.id.to_string()),
        "归档后仍在程序页列表里"
    );
    let archived_ids: Vec<String> = db_apps::list_archived_apps(&conn)
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert!(
        archived_ids.contains(&app.id.to_string()),
        "已归档视图里查不到"
    );
    // 恢复：回到草稿（不是直接上架——重新上架得用户满意后再定）
    let back = unarchive_core(&conn, &app.id, 3000).unwrap();
    assert_eq!(back.status, mc::STATUS_DRAFT);
    assert_eq!(back.enabled_at, None, "恢复成草稿不该留着上架时间");
    assert!(db_apps::list_active_apps(&conn)
        .unwrap()
        .iter()
        .any(|a| a.id == app.id));
}

#[test]
fn archive_is_idempotent_and_unarchive_rejects_non_archived() {
    let (_tmp, conn) = temp_db();
    let app = seed_enabled(&conn, "roster");
    archive_core(&conn, &app.id, 1000).unwrap();
    // 重复点归档：幂等返回，不报错；且是 no-op 不该动 updated_at
    //（动了会让「按最近更新排序」凭空跳一次）
    let again = archive_core(&conn, &app.id, 1500).unwrap();
    assert_eq!(again.status, mc::STATUS_ARCHIVED);
    assert_eq!(again.updated_at, 1000);
    // 未归档的程序不受理恢复（点了恢复但它本来就在页面上，是 bug 不是正常路径）
    let other = seed_enabled(&conn, "other");
    assert!(unarchive_core(&conn, &other.id, 2000).is_err());
    assert!(unarchive_core(&conn, "no-such-app", 2000).is_err());
}

/// 删干净是「库三张表 + 目录」全没，不是「列表里看不见了」。
///
/// 三张表之间没有外键：只删 apps 主行的话，app_versions / app_runs 会留下
/// 两堆永远没人再读的孤儿行（症状是库一直在涨、程序页却空空如也），
/// 而目录留着更是白占磁盘——用户点删除就是冲着腾地方来的。
#[test]
fn delete_removes_all_three_tables_and_the_directory() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    db_apps::record_app_run(&conn, &id, 2, "ok").unwrap();
    std::fs::create_dir_all(dir.join("input")).unwrap();
    std::fs::write(dir.join("input").join("本月.xlsx"), b"x").unwrap();
    archive_core(&conn, &id, 3).unwrap();

    delete_core(&dir, &conn, &id).unwrap();

    assert!(!dir.exists(), "程序目录没删干净（用户要的腾地方没兑现）");
    for table in ["apps", "app_versions", "app_runs"] {
        let n: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE app_id = ?1"),
                [&id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| {
                conn.query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE id = ?1"),
                    [&id],
                    |r| r.get(0),
                )
                .unwrap()
            });
        assert_eq!(n, 0, "{table} 里还留着 {id} 的行（无外键，不会自动级联）");
    }
    assert!(db_apps::list_archived_apps(&conn).unwrap().is_empty());
}

/// 删除只对已归档程序开放——「先归档、再删除」这两步不能被一次点击跳过。
///
/// 顺着跳过的话，程序页上那个垃圾桶就等于真删除，而那里的代码多半是 AI 写的；
/// 归档这一步存在的全部意义就是当那道安全网。
#[test]
fn delete_rejects_non_archived_and_unknown_ids() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("roster");
    let app = seed_enabled(&conn, "roster");
    std::fs::create_dir_all(&dir).unwrap();

    let err = delete_core(&dir, &conn, &app.id).expect_err("未归档程序不得直接删");
    assert!(
        matches!(err, CommandError::DomainRule { .. }),
        "该走业务拒绝而不是 IO/未找到错误：{err:?}"
    );
    assert!(dir.exists(), "被拒的程序目录不该被动过");
    assert!(db_apps::get_app(&conn, &app.id).unwrap().is_some());

    assert!(delete_core(&dir, &conn, "ghost").is_err(), "不存在的 id");
}

/// 目录早就不在了也算删成功。
///
/// 程序目录可能被用户手工清过、或从别处同步来一份只有元数据的行；
/// 那正是我们要的终态。为此报错会让「删除」在真实数据上无故失败。
#[test]
fn delete_tolerates_an_already_missing_directory() {
    let (tmp, conn) = temp_db();
    let app = seed_enabled(&conn, "ghost-dir");
    archive_core(&conn, &app.id, 1).unwrap();
    let dir = tmp.path().join("never-existed");

    delete_core(&dir, &conn, &app.id).expect("目录缺失是目标状态，不是失败");
    assert!(db_apps::get_app(&conn, &app.id).unwrap().is_none());
}

#[test]
fn update_requires_existing_and_rejects_archived_and_bad_status() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    // 不存在的 id
    let err = update_app_core(
        &dir,
        &conn,
        "ghost",
        Some("x"),
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap_err();
    assert!(matches!(err, CommandError::TaskNotFound(_)), "{err}");
    // archived 拒绝改
    conn.execute("UPDATE apps SET status='archived' WHERE id='wage'", [])
        .unwrap();
    let err = update_app_core(
        &dir,
        &conn,
        "wage",
        Some("x"),
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap_err();
    assert!(err.to_string().contains("已归档"), "{err}");
    // status=archived 从工具口拒绝
    conn.execute("UPDATE apps SET status='draft' WHERE id='wage'", [])
        .unwrap();
    let err = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[],
        None,
        None,
        None,
        None,
        None,
        Some("archived"),
        None,
        2,
    )
    .unwrap_err();
    assert!(err.to_string().contains("draft / enabled"), "{err}");
}

#[test]
fn update_rejects_noop_and_bad_extra_paths() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    // 无任何变更
    let err = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap_err();
    assert!(err.to_string().contains("没有任何变更"), "{err}");
    // 附加文件越出 data/
    let err = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[("input/steal.csv".to_string(), "x".to_string())],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap_err();
    assert!(err.to_string().contains("data/"), "{err}");
    // 穿越
    let err = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[("data/../../evil".to_string(), "x".to_string())],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap_err();
    assert!(err.to_string().contains("相对路径"), "{err}");
    // 合法 data/ 文件落盘
    update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[("data/roster.csv".to_string(), "a,b\n1,2".to_string())],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("data/roster.csv")).unwrap(),
        "a,b\n1,2"
    );
}

// ── 版本快照 ──

#[test]
fn versions_captured_on_create_update_restore() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].version, 1);
    assert_eq!(metas[0].source, "create");
    update_app_core(
        &dir,
        &conn,
        "wage",
        Some("print('v2')"),
        &[],
        None,
        None,
        None,
        None,
        None,
        Some("enabled"),
        None,
        2,
    )
    .unwrap();
    let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
    assert_eq!(metas.len(), 2);
    assert_eq!(metas[0].version, 2);
    assert_eq!(metas[0].source, "update");
    // 回滚到 v1：内容回旧版、生命周期（enabled）保留、本身记 v3
    restore_version_core(&dir, &conn, "wage", 1, 3).unwrap();
    let cur = db_apps::get_app(&conn, "wage").unwrap().unwrap();
    let v1 = db_apps::get_app_version(&conn, "wage", 1).unwrap().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.py")).unwrap(),
        v1.script
    );
    assert_eq!(cur.name, v1.name);
    assert_eq!(cur.status, "enabled", "回滚内容不回滚生命周期");
    assert_eq!(cur.enabled_at, Some(2));
    let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
    assert_eq!(metas.len(), 3);
    assert_eq!(metas[0].version, 3);
    assert_eq!(metas[0].source, "restore");
    // 回滚后可再取全文
    let v3 = db_apps::get_app_version(&conn, "wage", 3).unwrap().unwrap();
    assert_eq!(v3.script, v1.script);
}

#[test]
fn restore_rejects_missing_version_or_app() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    let err = restore_version_core(&dir, &conn, "wage", 99, 2).unwrap_err();
    assert!(matches!(err, CommandError::TaskNotFound(_)), "{err}");
    let err = restore_version_core(&dir, &conn, "ghost", 1, 2).unwrap_err();
    assert!(matches!(err, CommandError::TaskNotFound(_)), "{err}");
}

#[test]
fn restore_rejects_archived_app() {
    // 归档 = 先别动它：回滚会整体重写脚本与 manifest，与 update 同拒；
    // 恢复（unarchive）后才能回滚
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    archive_core(&conn, "wage", 2).unwrap();
    let err = restore_version_core(&dir, &conn, "wage", 1, 3).unwrap_err();
    assert!(matches!(err, CommandError::DomainRule { .. }), "{err}");
    // 恢复后同一版本可回滚
    unarchive_core(&conn, "wage", 4).unwrap();
    restore_version_core(&dir, &conn, "wage", 1, 5).unwrap();
}

#[test]
fn trim_keeps_recent_versions_and_version_number_keeps_growing() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    for i in 2..=5 {
        update_app_core(
            &dir,
            &conn,
            "wage",
            Some(&format!("print('v{i}')")),
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            i,
        )
        .unwrap();
    }
    db_apps::trim_app_versions(&conn, "wage", 3).unwrap();
    let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
    let versions: Vec<i64> = metas.iter().map(|m| m.version).collect();
    assert_eq!(versions, vec![5, 4, 3], "滚动保留最近 3 版");
    // 下一版本号继续增长，不因裁剪回退
    assert_eq!(db_apps::next_app_version(&conn, "wage").unwrap(), 6);
}

// ── validate_input_schema ──

#[test]
fn input_schema_accepts_folder_and_valid_form() {
    assert!(validate_input_schema(r#"{"kind":"folder"}"#).is_ok());
    assert!(validate_input_schema(
        r#"{"kind":"form","fields":[{"key":"month","label":"月份","type":"date","required":true}]}"#
    )
    .is_ok());
    // 字段缺 type 默认 text
    assert!(validate_input_schema(r#"{"kind":"form","fields":[{"key":"a"}]}"#).is_ok());
}

#[test]
fn input_schema_rejects_bad_kind_and_field_types_and_dup_keys() {
    assert!(validate_input_schema("not json").is_err());
    assert!(validate_input_schema(r#"{"kind":"wizard"}"#).is_err());
    assert!(validate_input_schema(r#"{"kind":"form"}"#).is_err());
    assert!(
        validate_input_schema(r#"{"kind":"form","fields":[{"key":"a","type":"blob"}]}"#).is_err()
    );
    assert!(
        validate_input_schema(r#"{"kind":"form","fields":[{"key":"a"},{"key":"a"}]}"#).is_err()
    );
    assert!(validate_input_schema(r#"{"kind":"form","fields":[{"key":"  "}]}"#).is_err());
}

// ── normalize_input_schema ──

#[test]
fn normalize_defaults_to_folder_and_accepts_object_or_string() {
    assert_eq!(
        normalize_input_schema(None).unwrap(),
        r#"{"kind":"folder"}"#
    );
    let obj: serde_json::Value =
        serde_json::from_str(r#"{"kind":"form","fields":[{"key":"a","type":"number"}]}"#).unwrap();
    assert!(normalize_input_schema(Some(&obj)).is_ok());
    let bad: serde_json::Value = serde_json::from_str(r#"{"kind":"x"}"#).unwrap();
    assert!(normalize_input_schema(Some(&bad)).is_err());
}

// ── 声明的运行时长 ──

/// 声明值的三态：未声明 / 0（取消）/ N 秒；超上限的钳到天花板（落盘=生效）
#[test]
fn declared_timeout_normalizes_and_clamps() {
    assert_eq!(normalize_declared_timeout(None), None);
    assert_eq!(normalize_declared_timeout(Some(0)), None, "0 = 取消声明");
    assert_eq!(normalize_declared_timeout(Some(1800)), Some(1800));
    assert_eq!(
        normalize_declared_timeout(Some(9999)),
        Some(crate::bot::constants::APPS_MAX_TIMEOUT_SECS),
        "超上限的声明要钳到天花板"
    );
    assert!(declared_timeout_clamped(Some(9999)));
    assert!(!declared_timeout_clamped(Some(600)));
    assert!(!declared_timeout_clamped(None));
}

/// 创建时的声明落库后可读回；未声明 = NULL（运行时走内置默认）
#[test]
fn create_stores_declared_timeout_and_reads_back() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("stt");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("stt");
    let app = create_app_core(
        &dir,
        &conn,
        &id,
        &name,
        &desc,
        &icon,
        &schema,
        &script,
        &files,
        &manual,
        Some(1800),
        1,
    )
    .unwrap();
    assert_eq!(app.timeout_secs, Some(1800));
    let stored = db_apps::get_app(&conn, "stt").unwrap().unwrap();
    assert_eq!(stored.timeout_secs, Some(1800), "声明值要能从库里读回来");
}

/// 更新语义：不传 = 保持不变；传 0 = 取消声明；传 N = 改声明
#[test]
fn update_timeout_keeps_clears_and_sets() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir,
        &conn,
        &id,
        &name,
        &desc,
        &icon,
        &schema,
        &script,
        &files,
        &manual,
        Some(900),
        1,
    )
    .unwrap();
    // 不传 timeoutSecs = 保持不变（本次改的是名字，所以是合法更新）
    let kept = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[],
        Some("新名字"),
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap();
    assert_eq!(kept.timeout_secs, Some(900));
    assert_eq!(kept.name, "新名字");
    // 传 N = 改声明
    let changed = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        Some(1200),
        3,
    )
    .unwrap();
    assert_eq!(changed.timeout_secs, Some(1200));
    // 传 0 = 取消声明（回落内置默认）
    let cleared = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        Some(0),
        4,
    )
    .unwrap();
    assert_eq!(cleared.timeout_secs, None);
}

/// 只改声明也算「有变更」（否则改声明会被「没有任何变更」那道闸误拒）
#[test]
fn timeout_only_change_is_accepted() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, None, 1,
    )
    .unwrap();
    let next = update_app_core(
        &dir,
        &conn,
        "wage",
        None,
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        Some(300),
        2,
    )
    .unwrap();
    assert_eq!(next.timeout_secs, Some(300));
}

/// 回滚回的是内容，不顺手改掉运行预算（声明不进版本快照）
#[test]
fn rollback_keeps_declared_timeout() {
    let (tmp, conn) = temp_db();
    let dir = tmp.path().join("wage");
    let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
    create_app_core(
        &dir,
        &conn,
        &id,
        &name,
        &desc,
        &icon,
        &schema,
        &script,
        &files,
        &manual,
        Some(600),
        1,
    )
    .unwrap();
    update_app_core(
        &dir,
        &conn,
        "wage",
        Some("print('v2')"),
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        2,
    )
    .unwrap();
    let restored = restore_version_core(&dir, &conn, "wage", 1, 3).unwrap();
    assert_eq!(restored.timeout_secs, Some(600), "回滚内容不改运行预算");
}

/// timeoutSecs 入参：缺省/ null = 未传；非整数一律拒（不静默忽略）
#[test]
fn timeout_field_accepts_integer_only() {
    let v = serde_json::json!({});
    assert_eq!(timeout_field(&v).unwrap(), None);
    let v = serde_json::json!({"timeoutSecs": null});
    assert_eq!(timeout_field(&v).unwrap(), None);
    let v = serde_json::json!({"timeoutSecs": 600});
    assert_eq!(timeout_field(&v).unwrap(), Some(600));
    let v = serde_json::json!({"timeoutSecs": 0});
    assert_eq!(timeout_field(&v).unwrap(), Some(0));
    let v = serde_json::json!({"timeoutSecs": "600"});
    assert!(timeout_field(&v).is_err(), "字符串时长要报错而不是猜");
    let v = serde_json::json!({"timeoutSecs": -1});
    assert!(timeout_field(&v).is_err(), "负数时长要报错");
    let v = serde_json::json!({"timeoutSecs": 1.5});
    assert!(timeout_field(&v).is_err(), "小数时长要报错");
}
