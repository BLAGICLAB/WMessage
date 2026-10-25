use super::*;

/// files 参数解析：去重保序、空白丢弃、isDir 读取、超 10 截断标记
#[test]
fn parse_task_files_arg_dedup_cap() {
    // 正常解析 + isDir
    let v = serde_json::json!({"files": [
        {"path": "/a/1.pdf", "isDir": false},
        {"path": "/a/dir", "isDir": true},
    ]});
    let (files, truncated) = parse_task_files_arg(&v).unwrap();
    assert_eq!(files.len(), 2);
    assert!(!files[0].is_dir && files[1].is_dir);
    assert!(!truncated);

    // 去重保序 + 空路径丢弃
    let v = serde_json::json!({"files": [
        {"path": "/a/1.pdf"}, {"path": "/a/1.pdf"}, {"path": "  "}, {"path": "/a/2.pdf"},
    ]});
    let (files, _) = parse_task_files_arg(&v).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "/a/1.pdf");
    assert_eq!(files[1].path, "/a/2.pdf");

    // 超上限：12 → 10 且 truncated=true
    let many: Vec<serde_json::Value> = (0..12)
        .map(|i| serde_json::json!({"path": format!("/f/{i}.txt"), "isDir": false}))
        .collect();
    let v = serde_json::json!({"files": many});
    let (files, truncated) = parse_task_files_arg(&v).unwrap();
    assert_eq!(files.len(), crate::db::MAX_TASK_FILES, "超 10 截断");
    assert_eq!(files[9].path, "/f/9.txt", "保序截前 10");
    assert!(truncated, "超上限必须标记 truncated");

    // 无 files 字段 → None（不动绑定）；空数组 → Some(空)（清除绑定）
    assert!(parse_task_files_arg(&serde_json::json!({"title": "x"})).is_none());
    let (files, truncated) = parse_task_files_arg(&serde_json::json!({"files": []})).unwrap();
    assert!(files.is_empty() && !truncated);
}

/// 模型来源 files 仅放行 AI_Gen_Files 内已存在条目（文件+文件夹），
/// is_dir 以真实 metadata 校正；白名单外/解析失败一律丢
#[test]
fn sanitize_task_files_whitelists_dirs_and_corrects_isdir() {
    let tmp = tempfile::tempdir().unwrap();
    let gen = tmp.path().join("AI_Gen_Files");
    std::fs::create_dir_all(&gen).unwrap();
    let inside = gen.join("report.docx");
    std::fs::write(&inside, b"x").unwrap();
    let inside_dir = gen.join("bundle");
    std::fs::create_dir_all(&inside_dir).unwrap();
    let outside = tmp.path().join("secret.txt");
    std::fs::write(&outside, b"s").unwrap();
    let gen_canon = std::fs::canonicalize(&gen).unwrap();

    let files = vec![
        crate::db::TaskFile {
            path: inside.to_string_lossy().to_string(),
            is_dir: false,
        },
        // 白名单内目录：放行（模型此前不给绑目录）
        crate::db::TaskFile {
            path: inside_dir.to_string_lossy().to_string(),
            is_dir: true,
        },
        // 文件谎报 isDir=true → 按 metadata 校正回 false（同路径重复项仅测 sanitize 不去重）
        crate::db::TaskFile {
            path: inside.to_string_lossy().to_string(),
            is_dir: true,
        },
        crate::db::TaskFile {
            path: outside.to_string_lossy().to_string(),
            is_dir: false,
        },
        // 白名单外目录：照旧丢
        crate::db::TaskFile {
            path: tmp.path().to_string_lossy().to_string(),
            is_dir: true,
        },
        // 不存在的路径也丢
        crate::db::TaskFile {
            path: gen.join("nope.txt").to_string_lossy().to_string(),
            is_dir: false,
        },
    ];
    let (kept, dropped) = sanitize_task_files_in(Some(&gen_canon), files);
    assert_eq!(kept.len(), 3, "白名单内文件+目录保留，谎报被校正");
    assert!(!kept[0].is_dir, "文件 is_dir=false");
    assert!(kept[1].is_dir, "白名单内目录保留");
    assert!(!kept[2].is_dir, "谎报目录的文件被 metadata 校正为文件");
    assert_eq!(dropped, 3);

    // gen 目录不可用（None）→ 全丢（fail-closed）
    let (kept, dropped) = sanitize_task_files_in(
        None,
        vec![crate::db::TaskFile {
            path: inside.to_string_lossy().to_string(),
            is_dir: false,
        }],
    );
    assert!(kept.is_empty() && dropped == 1);
}

/// 绑定目标解析：taskId 直查、title 精确匹配唯一卡、0 张拒（不得新建）、
/// 多张同名拒（改用 taskId）、回收站卡不可达
#[test]
fn resolve_task_key_requires_existing_unique_card() {
    let mk = |id: &str, title: &str, deleted: bool| crate::db::Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: id.to_string(),
        title: title.to_string(),
        due: None,
        note: None,
        tags: None,
        files: None,
        column: TaskStatus::Todo,
        subtasks: None,
        completed_at: None,
        archived: None,
        deleted_at: if deleted { Some(1) } else { None },
        collapsed: None,
        order: None,
        updated_at: None,
        bot_assigned: None,
        assignee: None,
        budget: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: None,
        created_at: None,
        expected_updated_at: None,
    };
    let tasks = vec![
        mk("t1", "写周报", false),
        mk("t2", "写周报", false),
        mk("t3", "已删的卡", true),
    ];
    // Task 未实现 Debug，unwrap_err 不可用——match 取错误
    let expect_err = |r: Result<&crate::db::Task, String>| -> String {
        match r {
            Ok(_) => panic!("应拒绝"),
            Err(e) => e,
        }
    };
    // taskId 直查
    assert_eq!(resolve_task_key(&tasks, "t1").unwrap().id, "t1");
    // 回收站卡按不存在处理
    let err = expect_err(resolve_task_key(&tasks, "t3"));
    assert!(err.contains("不存在"), "got: {err}");
    // 回收站卡不计入 title 匹配 → 0 张 → 拒（不能新建卡）
    let err = expect_err(resolve_task_key(&tasks, "title:已删的卡"));
    assert!(err.contains("不能新建卡"), "got: {err}");
    // title 唯一命中
    let tasks2 = vec![mk("t9", "唯一的卡", false)];
    assert_eq!(
        resolve_task_key(&tasks2, "title:唯一的卡").unwrap().id,
        "t9"
    );
    // title 0 张 → 拒（不能新建卡）
    let err = expect_err(resolve_task_key(&tasks, "title:不存在的卡"));
    assert!(err.contains("不能新建卡"), "got: {err}");
    // title 多张同名 → 拒（改用 taskId）
    let err = expect_err(resolve_task_key(&tasks, "title:写周报"));
    assert!(err.contains("taskId"), "got: {err}");
}

/// tool.return 审计 kv：files_count=原始条数（截断前）、truncated 标记；无 files 不补 kv
#[test]
fn files_audit_kv_counts_raw_and_flags_truncation() {
    let args = r#"{"files":[{"path":"/a"},{"path":"/b"}]}"#;
    assert_eq!(files_audit_kv(args), Some((2, false)));
    let many: Vec<String> = (0..11)
        .map(|i| format!("{{\"path\":\"/f/{i}\"}}"))
        .collect();
    let args = format!("{{\"files\":[{}]}}", many.join(","));
    assert_eq!(files_audit_kv(&args), Some((11, true)), "超 10 → truncated");
    assert_eq!(files_audit_kv(r#"{"title":"x"}"#), None);
}
