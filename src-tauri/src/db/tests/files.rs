//! 目录拷贝防护与任务文件绑定（files 列读写回环、去重截断）。

use super::*;

// ── APW-02a: copy_dir_recursive 防护 ──

/// dst 已存在 → 返 Err（保守语义，不改现有“dst 不存在”行为）
#[test]
fn copy_dir_recursive_rejects_existing_dst() {
    let dir = std::env::temp_dir().join(format!("wm-copy-dst-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("file.txt"), b"x").unwrap();
    let result = copy_dir_recursive(&src, &dst);
    assert!(result.is_err(), "dst 已存在应返 Err，实际 {result:?}");
    assert!(dst.exists(), "dst 路径应原样未变（无半截内容）");
    fs::remove_dir_all(&dir).ok();
}

/// src 是 symlink → 返 Err（read_dir 跟读目标 = move 语义失控）
///
/// symlink 的创建是 unix-only API——Windows 上没有对应物，这个用例不适用。
#[cfg(unix)]
#[test]
fn copy_dir_recursive_rejects_symlink_src() {
    let dir = std::env::temp_dir().join(format!("wm-copy-sym-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let real_src = dir.join("real_src");
    fs::create_dir_all(&real_src).unwrap();
    fs::write(real_src.join("file.txt"), b"x").unwrap();
    let sym_src = dir.join("sym_src");
    std::os::unix::fs::symlink(&real_src, &sym_src).unwrap();
    let dst = dir.join("dst");
    let result = copy_dir_recursive(&sym_src, &dst);
    assert!(result.is_err(), "src 是 symlink 应返 Err，实际 {result:?}");
    fs::remove_dir_all(&dir).ok();
}

/// files 列读写回环：upsert 多文件 → load_all 原样读回
#[test]
fn files_roundtrip_via_upsert_load() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, conn) = setup_tasks_db();
    let mut t = mk_task("t1", "多文件");
    t.files = Some(vec![
        TaskFile {
            path: "/a/1.pdf".into(),
            is_dir: false,
        },
        TaskFile {
            path: "/a/2.docx".into(),
            is_dir: false,
        },
    ]);
    upsert_tasks(&conn, &[t]).unwrap();
    let loaded = &load_all(&conn).unwrap()[0];
    assert_eq!(loaded.files.as_deref().unwrap().len(), 2);
    assert_eq!(loaded.files.as_deref().unwrap()[1].path, "/a/2.docx");
    fs::remove_dir_all(&dir).ok();
}

/// effective_files：返回 files 绑定；未绑定为空
#[test]
fn effective_files_returns_bound_files() {
    let mut t = mk_task("t1", "x");
    assert!(t.effective_files().is_empty(), "都没绑 → 空");
    t.files = Some(vec![TaskFile {
        path: "/tmp/new.pdf".into(),
        is_dir: false,
    }]);
    assert_eq!(
        t.effective_files(),
        vec![TaskFile {
            path: "/tmp/new.pdf".into(),
            is_dir: false
        }],
        "files 非空 → 原样返回"
    );
}

/// resolve_task_files：去重保序、空路径丢弃、超 MAX_TASK_FILES 截断、
/// 目录经 fs::metadata 判 isDir、不存在路径按文件处理
#[test]
fn resolve_task_files_dedup_cap_and_isdir() {
    let dir = std::env::temp_dir().join(format!("wm-resolve-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let dir_str = dir.to_string_lossy().to_string();

    // 目录判定 + 去重保序 + 不存在路径按文件
    let out = resolve_task_files(vec![
        dir_str.clone(),
        "/no/such/file.txt".into(),
        dir_str.clone(), // 重复
        "  ".into(),     // 空白丢弃
    ]);
    assert_eq!(out.len(), 2, "去重 + 空白丢弃");
    assert!(out[0].is_dir, "真实目录 isDir=true");
    assert_eq!(out[1].path, "/no/such/file.txt");
    assert!(!out[1].is_dir, "不存在路径按文件处理");

    // 上限截断：12 个 → 10 个，保序
    let many: Vec<String> = (0..12).map(|i| format!("/f/{i}.txt")).collect();
    let capped = resolve_task_files(many);
    assert_eq!(capped.len(), MAX_TASK_FILES, "超 10 截断");
    assert_eq!(capped[9].path, "/f/9.txt", "保序截断前 10 个");
    fs::remove_dir_all(&dir).ok();
}

/// upsert_files_to_task 统一绑定内核：文件追加、文件夹槽替换、小写路径去重、
/// 10 封顶、空追加无操作、只增不减
#[test]
fn upsert_files_to_task_additive_with_folder_slot() {
    let tf = |path: &str, is_dir: bool| TaskFile {
        path: path.to_string(),
        is_dir,
    };
    let mut t = mk_task("t1", "x");

    // 空追加 = 无操作（files 仍 None，只增不减）
    let (applied, overflow) = upsert_files_to_task(&mut t, vec![]);
    assert_eq!((applied, overflow), (0, 0));
    assert!(t.files.is_none(), "空追加不得写回空绑定");

    // 文件追加 + 小写路径去重（同路径不同大小写 = 同一条目）
    let (applied, overflow) = upsert_files_to_task(
        &mut t,
        vec![tf("/d/报.docx", false), tf("/D/报.DOCX", false)],
    );
    assert_eq!((applied, overflow), (1, 0), "小写归一去重");
    assert_eq!(t.files.as_deref().unwrap().len(), 1);

    // 文件夹槽：首个文件夹入列
    let (applied, _) = upsert_files_to_task(&mut t, vec![tf("/d/归档", true)]);
    assert_eq!(applied, 1);
    // 不同路径文件夹 → 替换占原位，文件条目与顺序不动
    let (applied, _) = upsert_files_to_task(&mut t, vec![tf("/e/其他", true)]);
    assert_eq!(applied, 1, "文件夹替换计为生效");
    let files = t.files.as_deref().unwrap();
    assert_eq!(files.len(), 2, "文件夹替换不增长列表");
    assert_eq!(files[1].path, "/e/其他", "新文件夹占原槽位");
    assert_eq!(files[0].path, "/d/报.docx", "文件条目不动");
    // 同路径文件夹重复绑 → 无操作
    let (applied, overflow) = upsert_files_to_task(&mut t, vec![tf("/E/其他", true)]);
    assert_eq!((applied, overflow), (0, 0));

    // 10 封顶：文件溢出计数；满员时文件夹替换仍生效
    let mut full = mk_task("t2", "y");
    let fill: Vec<TaskFile> = (0..9)
        .map(|i| tf(&format!("/f/{i}.txt", i = i), false))
        .collect();
    upsert_files_to_task(&mut full, fill);
    upsert_files_to_task(&mut full, vec![tf("/f/-dir", true)]);
    let (applied, overflow) = upsert_files_to_task(&mut full, vec![tf("/f/new.txt", false)]);
    assert_eq!((applied, overflow), (0, 1), "满员文件溢出");
    let (applied, overflow) = upsert_files_to_task(&mut full, vec![tf("/f/替换目录", true)]);
    assert_eq!((applied, overflow), (1, 0), "满员文件夹替换仍生效");
    assert_eq!(full.files.as_deref().unwrap().len(), 10, "替换不增长");
    assert_eq!(
        full.files
            .as_deref()
            .unwrap()
            .iter()
            .find(|f| f.is_dir)
            .unwrap()
            .path,
        "/f/替换目录"
    );
}

/// 旧版本工作区链接存 label/target 字段，新版本 displayName/targetUri，
/// serde alias 保证旧数据无缝读取。
#[test]
fn workspace_link_legacy_fields_compat() {
    let old = r#"[{"id":"l1","label":"Desktop","target":"/Users/x/Desktop","kind":"folder"}]"#;
    let links: Vec<WorkspaceLink> = serde_json::from_str(old).unwrap();
    assert_eq!(links[0].display_name, "Desktop");
    assert_eq!(links[0].target_uri, "/Users/x/Desktop");
    assert_eq!(links[0].kind, "folder");

    let new = r#"[{"id":"l2","displayName":"别名","targetUri":"https://x.com","kind":"url"}]"#;
    let links2: Vec<WorkspaceLink> = serde_json::from_str(new).unwrap();
    assert_eq!(links2[0].display_name, "别名");
    assert_eq!(links2[0].target_uri, "https://x.com");

    // 序列化输出新字段名（camelCase）
    let out = serde_json::to_string(&links2).unwrap();
    assert!(out.contains("displayName") && out.contains("targetUri"));
}
