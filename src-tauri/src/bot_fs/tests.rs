use super::read::{grep_file_into, grep_target, GrepTarget};
use super::*;

/// 溢出根目录必须以精确路径钉在读白名单（canonical 后与 overflow::root
/// 完全相等）：路径写歪时 canonicalize 失败被静默跳过、不报错——
/// 指针文件从此读不回，比直接截断还糟，且没有任何显式失败可查。
#[test]
fn mutant_m7_preview_overflow_root_pinned_in_read_allowlist() {
    let _g = crate::overflow::OVERFLOW_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let app = tauri::test::mock_app();
    let root = crate::overflow::root(app.handle());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut dirs: Vec<PathBuf> = Vec::new();
    append_session_read_dirs(app.handle(), None, &mut dirs);
    let expected = std::fs::canonicalize(&root).unwrap();
    assert!(
        dirs.contains(&expected),
        "读白名单必须含溢出根目录 {expected:?}，实际：{dirs:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// py 沙箱运行目录必须在读白名单里：脚本跑失败时留在里面，模型要读回
/// 「刚才到底跑了什么」就得能读——读不到就只能把整段脚本重写一遍。
///
/// 只加读侧：模型不能往沙箱里写。
#[test]
fn py_runs_root_pinned_in_read_allowlist() {
    let app = tauri::test::mock_app();
    let root = crate::bot_py::py_runs_root();
    // 只建不删：并行跑的真实 python 用例可能正在这个根下建目录，
    // 删根会连带删掉它们的运行目录。
    std::fs::create_dir_all(&root).expect("建 py 沙箱根目录");
    let mut dirs: Vec<PathBuf> = Vec::new();
    append_session_read_dirs(app.handle(), None, &mut dirs);
    let expected = std::fs::canonicalize(&root).unwrap();
    assert!(
        dirs.contains(&expected),
        "读白名单必须含 py 沙箱根目录 {expected:?}，实际：{dirs:?}"
    );
}

// N3-6：grep 上下文渲染

#[test]
fn render_context_zero_matches_old_format() {
    let lines = ["alpha", "beta", "gamma"];
    let out = render_context_hits("/tmp/a.txt", &lines, &[1], 0);
    assert_eq!(
        out,
        vec!["/tmp/a.txt:2: beta"],
        "context=0 应与旧格式逐字一致"
    );
}

#[test]
fn render_context_windows_and_separator() {
    let lines = ["l0", "l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9"];
    // 两个相距远的命中（1 与 8），context=1 → 两块，中间 --
    let out = render_context_hits("/t/f.txt", &lines, &[1, 8], 1);
    assert_eq!(
        out,
        vec![
            "/t/f.txt-1- l0",
            "/t/f.txt:2: l1",
            "/t/f.txt-3- l2",
            "--",
            "/t/f.txt-8- l7",
            "/t/f.txt:9: l8",
            "/t/f.txt-10- l9",
        ]
    );
}

#[test]
fn render_context_merges_overlapping_windows() {
    let lines = ["l0", "l1", "l2", "l3", "l4"];
    // 命中 1 与 2，context=1：窗口 [0..2] 与 [1..3] 重叠 → 合并一块、无 --
    let out = render_context_hits("/t/f.txt", &lines, &[1, 2], 1);
    assert_eq!(
        out,
        vec![
            "/t/f.txt-1- l0",
            "/t/f.txt:2: l1",
            "/t/f.txt:3: l2",
            "/t/f.txt-4- l3",
        ]
    );
    assert!(!out.contains(&"--".to_string()), "合并块不得有分隔符");
}

#[test]
fn render_context_clamps_at_file_edges() {
    let lines = ["only"];
    let out = render_context_hits("/t/f.txt", &lines, &[0], 5);
    assert_eq!(out, vec!["/t/f.txt:1: only"], "窗口越界应夹到文件边界");
}

// edit 三级匹配 / write 校验 / 可写根集合

#[test]
fn apply_edit_exact_unique_and_multi_hit() {
    let content = "fn a() {}\nfn b() {}\n";
    let old = "fn b() {}";
    let (out, mode, delta, replacements) =
        try_apply_edit(content, old, "fn b() -> i32 { 1 }", false).unwrap();
    assert_eq!(mode, EditMode::Exact);
    assert_eq!(delta, 0);
    assert_eq!(replacements, 1);
    assert!(out.contains("fn b() -> i32 { 1 }"), "{out}");
    // 多处命中：两个相同函数体
    let dup = "x = 1;\nx = 1;\n";
    let (kind, hint) = try_apply_edit(dup, "x = 1;", "x = 2;", false).unwrap_err();
    assert_eq!(kind, EditErrorKind::MultiHit);
    assert!(hint.contains("2 处"), "{hint}");
}

#[test]
fn apply_edit_whitespace_fallback_and_not_found() {
    // 原文行尾有尾随空格 + CRLF：精确失败、空白容错命中
    let content = "fn a() {\r\n    return 1;   \r\n}\r\n";
    let old = "fn a() {\n    return 1;\n}";
    let (out, mode, delta, replacements) =
        try_apply_edit(content, old, "fn a() {\r\n    return 2;\r\n}", false).unwrap();
    assert_eq!(mode, EditMode::WhitespaceTolerant);
    assert_eq!(delta, 0);
    assert_eq!(replacements, 1);
    assert!(out.contains("return 2;"), "{out}");
    // 空白容错也多处 → MultiHit
    let dup_ws = "x = 1;  \nx = 1;\n";
    let (kind, _) = try_apply_edit(dup_ws, "x = 1;", "x = 2;", false).unwrap_err();
    assert_eq!(kind, EditErrorKind::MultiHit);
    // 全失败 → NotFound + reflection 提示带文件开头
    let (kind, hint) = try_apply_edit("alpha\nbeta\n", "不存在的行", "x", false).unwrap_err();
    assert_eq!(kind, EditErrorKind::NotFound);
    assert!(hint.contains("read_text_file"), "{hint}");
    assert!(hint.contains("alpha"), "提示应带文件开头：{hint}");
}

#[test]
fn apply_edit_preserves_crlf_and_trailing_newline() {
    let content = "a\r\nold\r\nb\r\n";
    let (out, _, _, _) = try_apply_edit(content, "old", "new", false).unwrap();
    assert_eq!(out, "a\r\nnew\r\nb\r\n", "CRLF 文件替换后保持 CRLF");
    // 无结尾换行的文件重建后不引入结尾换行
    let content2 = "x\nold";
    let (out2, _, _, _) = try_apply_edit(content2, "old", "new", false).unwrap();
    assert_eq!(out2, "x\nnew");
}

/// 空白容错**不得**把文件原有的混合换行整体归一。
///
/// 原实现把全文按行重建再用一个换行符拼回去：文件里与本次编辑无关的
/// 那部分换行（这里是 "a" 后面的 LF）被改成主导换行符——一次与编辑
/// 无关的全文件改动，diff 全红。精确匹配路径的 replacen 不会有这问题。
#[test]
fn apply_edit_whitespace_fallback_keeps_mixed_eol_outside_the_edit() {
    // 行尾空白让精确匹配失配 → 走空白容错；文件本身是混合换行
    let content = "a\nold  \nb\r\nc\n";
    let (out, mode, _, _) = try_apply_edit(content, "old\nb", "X\nY", false).unwrap();
    assert_eq!(mode, EditMode::WhitespaceTolerant);
    assert_eq!(
        out, "a\nX\r\nY\r\nc\n",
        "编辑区间外的换行必须原样保留：{out:?}"
    );
}

/// 空白容错必须保留 newString 明确要求的尾换行。
///
/// 原实现按行重建（`new.lines()` 不含尾换行），替换区在文件末尾时，
/// 模型要求的尾换行直接消失——文件仍不以换行结尾。
#[test]
fn apply_edit_whitespace_fallback_keeps_requested_trailing_newline() {
    // 文件本身没有尾换行；替换区在文件末尾
    let content = "x\nold  \ny";
    let (out, _, _, _) = try_apply_edit(content, "old\ny", "new\n", false).unwrap();
    assert_eq!(out, "x\nnew\n", "newString 要求的尾换行不能被吃掉：{out:?}");
}

/// 路径类型探测必须三态：目录 / 不是目录 / 探测失败。
///
/// 分不清「不是目录」与「没探到」的话，授权粒度那处会朝**过度授权**
/// 方向失败：探测失败被判成文件 → 授权落到父目录，比用户选中的宽一层。
#[tokio::test]
async fn is_dir_probe_distinguishes_missing_from_not_a_dir() {
    let dir = std::env::temp_dir().join("wmessage-isdir-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let file = dir.join("a.txt");
    std::fs::write(&file, b"x").expect("写测试文件");
    assert_eq!(is_dir_async(&dir).await, Some(true), "目录应判 Some(true)");
    assert_eq!(
        is_dir_async(&file).await,
        Some(false),
        "文件应判 Some(false)"
    );
    assert_eq!(
        is_dir_async(&dir.join("nope")).await,
        None,
        "探不到（路径不存在）必须是 None，不能与「不是目录」混同"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 接线守卫：「始终允许」落盘授权前必须先探明路径类型，
/// 探测失败时**不落盘**（宁可不授权，也不朝宽的方向失败）。
#[test]
fn always_allow_does_not_persist_on_probe_failure() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_fs.rs"))
        .expect("应能读到自己");
    let start = src.find("ConfirmChoice::Always").expect("应有 Always 分支");
    let rest = &src[start..];
    let end = rest[1..]
        .find("ConfirmChoice::Deny")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    let branch = &rest[..end];
    assert!(
        branch.contains("None =>"),
        "Always 分支必须显式处理探测失败（None）"
    );
    assert!(
        branch.contains("本次授权未保存"),
        "探测失败必须不落盘授权（落盘就得先定粒度，而两种粒度此刻都只是猜）"
    );
}

/// 超大文件必须**先按上限读、再判体积**：整读入内存后才判上限，
/// 白名单内一个 GB 级日志就能把进程内存打爆。
///
/// 判据取「体积超限」而不是「非 UTF-8」：旧实现整读时先撞上非 UTF-8
/// 就报错，用户看到的是「不是文本」，看不到真因（文件太大、请拆分）。
#[test]
fn edit_rejects_oversized_file_by_size_not_by_encoding() {
    let dir = std::env::temp_dir().join("wmessage-edit-max-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let path = dir.join("big.txt");
    let mut body = vec![b'a'; EDIT_MAX_FILE_BYTES + 4096];
    body.push(0xFF); // 尾部一个非法字节：整读会先在这里失败
    std::fs::write(&path, &body).expect("写测试文件");

    let err = match edit_file_sync(path.clone(), "a".into(), "b".into(), false) {
        Ok(_) => panic!("超限文件必须被拒"),
        Err(e) => e,
    };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        err.1.contains("超过 edit_file 上限"),
        "应报体积超限（可操作指引），实际：{}",
        err.1
    );
}

// edit regex 模式（mode=regex 走 try_apply_edit use_regex=true 路径）

#[test]
fn apply_edit_regex_replace_all_and_capture_groups() {
    // 全部命中 + 捕获组引用
    let content = "foo 1 bar 2 foo 3\n";
    let (out, mode, _delta, replacements) =
        try_apply_edit(content, r"(\w+)\s+(\d+)", "$1=$2", true).unwrap();
    assert_eq!(mode, EditMode::Regex { matches: 3 });
    assert_eq!(replacements, 3);
    assert_eq!(out, "foo=1 bar=2 foo=3\n", "{out}");

    // 单行命中一次（regex 模式不走 unique 强制，1 处也算 replace）
    let single = "let x = 1;\n";
    let (out2, mode2, _, replacements2) =
        try_apply_edit(single, r"x\s*=\s*1", "x = 2", true).unwrap();
    assert_eq!(mode2, EditMode::Regex { matches: 1 });
    assert_eq!(replacements2, 1);
    assert_eq!(out2, "let x = 2;\n", "{out2}");
}

#[test]
fn apply_edit_regex_zero_match_and_invalid_pattern() {
    // 0 命中 → NotFound + 提示带文件开头
    let (kind, hint) = try_apply_edit("alpha\nbeta\n", r"zzz", "x", true).unwrap_err();
    assert_eq!(kind, EditErrorKind::NotFound);
    assert!(hint.contains("alpha"), "提示应带文件开头：{hint}");

    // 非法 regex（未闭合括号）→ NotFound（不 panic）
    let (kind, hint) = try_apply_edit("hello", r"(", "x", true).unwrap_err();
    assert_eq!(kind, EditErrorKind::NotFound);
    assert!(hint.contains("regex"), "提示应说明模式串非法：{hint}");
}

#[test]
fn apply_edit_regex_wipe_refused() {
    // regex 全清空文件 → 拒（fail-closed，提示走 write_file）
    let (kind, hint) = try_apply_edit("abc", r".*", "", true).unwrap_err();
    assert_eq!(kind, EditErrorKind::NotFound);
    assert!(hint.contains("清空"), "{hint}");
    assert!(hint.contains("write_file"), "{hint}");

    // 非空文件 regex 替换后非空 → 允许（即使删了大部分内容）
    let (out, mode, _, _) = try_apply_edit("abc\n", r"abc", "", true).unwrap();
    assert_eq!(mode, EditMode::Regex { matches: 1 });
    assert_eq!(out, "\n", "{out}");
}

#[test]
fn validate_write_content_rejects_nul_and_oversize() {
    assert!(validate_write_content("# ok\n").is_ok());
    assert!(
        validate_write_content("").is_ok(),
        "空文件合法（新建空文件）"
    );
    assert!(validate_write_content("a\0b").is_err(), "NUL 拒");
    let big = "A".repeat(2 * 1024 * 1024 + 1);
    assert!(validate_write_content(&big).is_err(), "超 2MB 拒");
}

#[test]
fn writable_raw_excludes_home_defaults() {
    // 关键安全断言：可写根不含桌面/下载/文档默认项（读白名单有、写白名单刻意没有）
    let raw = build_writable_raw(
        Some("/data/AI_Gen_Files".into()),
        vec!["/Users/me/project".into()],
        &["/Users/me/工作".to_string()],
    );
    assert_eq!(raw.len(), 3, "{raw:?}");
    assert!(!raw
        .iter()
        .any(|d| d.contains("Desktop") || d.contains("Downloads")));
}

#[test]
fn glob_match_basics() {
    assert!(glob_match("*.rs", "main.rs"));
    assert!(glob_match("*.rs", "a.b.rs"));
    assert!(!glob_match("*.rs", "main.ts"));
    assert!(glob_match("todo*", "todo_list"));
    assert!(glob_match("????.txt", "2024.txt"));
    assert!(!glob_match("????.txt", "20245.txt"));
    assert!(glob_match("*", "anything"));
    assert!(glob_match("", ""));
    assert!(!glob_match("", "x"));
    assert!(glob_match("a*c", "abc"));
    assert!(!glob_match("a*c", "abd"));
}

#[test]
fn expand_tilde_home() {
    // 走 home_dir() 跨平台解析（Windows 上 HOME 常缺失，不能直接 unwrap HOME）
    let home = home_dir().expect("dev/CI 机器应有可解析的用户主目录");
    assert_eq!(expand_tilde("~/x"), PathBuf::from(&home).join("x"));
    assert_eq!(expand_tilde("~"), PathBuf::from(&home));
    assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    // Windows 风格波浪号也展开（LLM 可能给 ~\Desktop）
    assert_eq!(expand_tilde("~\\x"), PathBuf::from(&home).join("x"));
}

/// walk 跳过符号链接——白名单目录内的软链
/// 指向外部时，grep 的内容读取跟随软链会把外部文件带进模型上下文
#[cfg(unix)]
#[test]
fn walk_skips_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // 真实文件 + 指向外部文件的软链 + 指向外部目录的软链
    std::fs::write(root.join("real.txt"), b"ok").unwrap();
    let outside =
        std::env::temp_dir().join(format!("wm_outside_{}.txt", uuid::Uuid::new_v4().simple()));
    std::fs::write(&outside, b"secret").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(std::env::temp_dir(), root.join("link_dir")).unwrap();

    let mut visited: Vec<String> = Vec::new();
    walk(root, &mut |p: &Path, _is_dir: bool| {
        visited.push(p.file_name().unwrap().to_string_lossy().to_string());
        true
    });
    assert!(visited.contains(&"real.txt".to_string()));
    assert!(
        !visited.contains(&"link.txt".to_string()),
        "软链文件必须跳过"
    );
    assert!(
        !visited.contains(&"link_dir".to_string()),
        "软链目录必须跳过"
    );
    // 软链目标的内容不应出现在遍历里
    assert!(!visited.iter().any(|n| n.starts_with("wm_outside_")));
    let _ = std::fs::remove_file(&outside);
}

/// HOME 缺失（Windows 绿色版常见）时须回退 USERPROFILE
#[test]
fn home_dir_fallbacks() {
    let os = |s: &str| Some(std::ffi::OsString::from(s));
    // HOME 优先
    assert_eq!(
        home_dir_from(os("/h"), os(r"C:\Users\u"), None, None),
        Some(PathBuf::from("/h"))
    );
    // HOME 缺失（或为空）→ USERPROFILE
    assert_eq!(
        home_dir_from(None, os(r"C:\Users\u"), None, None),
        Some(PathBuf::from(r"C:\Users\u"))
    );
    assert_eq!(
        home_dir_from(os(""), os(r"C:\Users\u"), None, None),
        Some(PathBuf::from(r"C:\Users\u"))
    );
    // 再退 HOMEDRIVE+HOMEPATH
    assert_eq!(
        home_dir_from(None, None, os("C:"), os(r"\Users\u")),
        Some(PathBuf::from(r"C:\Users\u"))
    );
    // 全空 → None
    assert_eq!(home_dir_from(None, None, None, None), None);
}

/// Windows canonicalize 的 \\?\ 前缀剥离：盘符形式剥掉，UNC 保留，普通路径不动
#[test]
fn strip_verbatim_prefix() {
    assert_eq!(
        strip_verbatim(PathBuf::from(r"\\?\C:\Users\u\Desktop\x.docx")),
        PathBuf::from(r"C:\Users\u\Desktop\x.docx")
    );
    assert_eq!(
        strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\x.docx")),
        PathBuf::from(r"\\?\UNC\server\share\x.docx")
    );
    assert_eq!(
        strip_verbatim(PathBuf::from("/Users/u/Desktop/x.docx")),
        PathBuf::from("/Users/u/Desktop/x.docx")
    );
}

/// 白名单判定核心：starts_with 是按路径分量比较，
/// 前缀相似的不同目录（/tmp/ab vs /tmp/abc）不会误判命中
#[test]
fn prefix_similar_dirs_not_confused() {
    let allowed = Path::new("/tmp/ab");
    assert!(Path::new("/tmp/ab/c").starts_with(allowed));
    assert!(Path::new("/tmp/ab").starts_with(allowed));
    assert!(!Path::new("/tmp/abc").starts_with(allowed));
    assert!(!Path::new("/tmp/ab2/x").starts_with(allowed));
}

/// 追加语义：allowedDirs 非空时内置默认（桌面/下载/文档）仍生效，
/// 任务卡绑定文件夹也在并集里（用户列表整体替换默认会导致授权弹窗
/// 「始终允许」写入一个目录后内置默认失效）
#[test]
fn merge_raw_dirs_appends_to_defaults() {
    let home = PathBuf::from("/home/u");
    let cfg = vec!["/data/work".to_string()];
    let task = vec!["/mnt/bound".to_string()];
    let raw = merge_raw_dirs(&cfg, Some(&home), task, Some("/data/gen".to_string()));
    assert_eq!(
        raw,
        vec![
            "/home/u/Desktop",
            "/home/u/Downloads",
            "/home/u/Documents",
            "/data/gen",
            "/mnt/bound",
            "/data/work"
        ]
    );
}

/// home 缺失（Windows 绿色版 HOME/USERPROFILE 全空兜底场景）时不产默认目录，
/// 但 AI 产物目录、任务卡绑定与用户配置仍在
#[test]
fn merge_raw_dirs_without_home() {
    let raw = merge_raw_dirs(
        &["/data/x".to_string()],
        None,
        vec!["/mnt/b".to_string()],
        Some("/data/gen".to_string()),
    );
    assert_eq!(raw, vec!["/data/gen", "/mnt/b", "/data/x"]);
    // gen 目录缺失（创建失败）时其余集合不受影响
    let raw = merge_raw_dirs(&["/data/x".to_string()], None, vec![], None);
    assert_eq!(raw, vec!["/data/x"]);
}

// ── 白名单逃逸（bot_fs 是「模型编造路径 / 前端 XSS 驱动任意文件读」的关键防线）──

/// 判定 = canonicalize + 分量前缀比较，两步缺一不可：
/// `..` 穿越必须被解析后在白名单外，前缀相似目录不得误判命中
#[test]
fn allowlist_rejects_traversal_and_prefix_similar_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let allowed = tmp.path().join("allowed");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
    std::fs::write(allowed.join("mine.txt"), b"mine").unwrap();

    let allow_dirs = vec![
        std::fs::canonicalize(&allowed).unwrap(),
        std::fs::canonicalize(tmp.path()).unwrap(), // 目录本身也可命中（授权粒度=目录）
    ];

    let inside = std::fs::canonicalize(allowed.join("mine.txt")).unwrap();
    assert!(is_within_allowlist(&inside, &allow_dirs), "白名单内应放行");

    // `..` 穿越：canonicalize 已解析 `..` → 落在白名单外 → 拒不命中
    let traversal = std::fs::canonicalize(allowed.join("../outside/secret.txt")).unwrap();
    assert!(
        !is_within_allowlist(&traversal, &[allow_dirs[0].clone()]),
        "`..` 穿越到白名单外必须被拒"
    );

    // 前缀相似目录（/x/allowed vs /x/allowed-evil）：分量比较不得误吞
    let sibling = tmp.path().join("allowed-evil");
    std::fs::create_dir_all(&sibling).unwrap();
    let sibling_c = std::fs::canonicalize(&sibling).unwrap();
    assert!(
        !is_within_allowlist(&sibling_c, &[allow_dirs[0].clone()]),
        "前缀相似目录不得误判命中"
    );
}

/// 软链逃逸（unix）：白名单目录内的软链指向外部 → canonicalize 跟随软链 →
/// 判定必须在白名单外（否则 grep_files 会把外部文件内容带进模型上下文）
#[cfg(unix)]
#[test]
fn allowlist_rejects_symlink_escape() {
    let tmp = tempfile::tempdir().unwrap();
    let allowed = tmp.path().join("allowed");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), allowed.join("link.txt")).unwrap();

    let allow_dirs = vec![std::fs::canonicalize(&allowed).unwrap()];
    let followed = std::fs::canonicalize(allowed.join("link.txt")).unwrap();
    assert!(
        !is_within_allowlist(&followed, &allow_dirs),
        "白名单内软链指向外部文件时必须拒（canonicalize 跟随软链后不在白名单内）"
    );
}

/// 【 C1b】始终允许该目录：授权粒度 = 用户输入原始父目录（不是 canonical.parent）
///
/// 攻击模型：用户输入 ~/allowed/sub/link.txt → link.txt 是软链 → /etc/passwd。
/// canonical 解析后 = /etc/passwd，canonical.parent() = /etc。
/// 旧实现用 canonical.parent() 会把 /etc/ 写进 allowedDirs（symlink escape）。
/// 新实现必须用 expand_tilde(p).parent()（用户输入视角）。
///
/// 验证方式：构造同样场景，断言 allowlist 增加的是用户输入目录 ~/allowed/sub，
/// 不是 /etc。直接测 resolve_with_perm 需要 AppHandle 复杂 mock；
/// 这里走「等价纯函数」路径，验证"dir 计算逻辑"本身。
#[cfg(unix)]
#[test]
fn always_allow_uses_user_input_dir_not_canonical_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let allowed = tmp.path().join("allowed");
    let sub = allowed.join("sub");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    // macOS firmlink：/var → /private/var，setup 即规范化，下方断言在 canonical 视角比较
    let outside = std::fs::canonicalize(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
    // sub/link.txt → /tmp/xxx/outside/secret.txt
    std::os::unix::fs::symlink(outside.join("secret.txt"), sub.join("link.txt")).unwrap();

    // 用户输入路径（与 resolve_with_perm 中"expanded" 同形）
    let user_input = sub.join("link.txt");
    let expanded = user_input.clone();
    let canonical = std::fs::canonicalize(&expanded).unwrap();

    // 旧实现（有 bug）：dir = canonical.parent() = /tmp/xxx/outside
    let old_dir = canonical
        .parent()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| canonical.clone());

    // 新实现（修复）：dir = expanded.parent()（用户输入视角）
    // expanded 是 ~/allowed/sub/link.txt，expanded.parent() = ~/allowed/sub
    let new_dir = expanded
        .parent()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| canonical.clone());

    // 旧值会等于 outside（escape 发生），新值必须等于 sub（不逃逸）
    assert_eq!(
        new_dir, sub,
        "新实现 dir 必须是 expanded.parent() = sub，不能是 canonical.parent() = outside"
    );
    assert_ne!(
        old_dir, sub,
        "sanity check：旧实现 dir = canonical.parent() 应等于 outside（演示攻击场景）"
    );
    assert_eq!(
        old_dir, outside,
        "sanity check：旧实现 dir 应该等于 outside（演示 canonical.parent 被换的事实）"
    );
}

/// 【 TOCTOU】canonicalize 与 read 之间，原路径被替换为同名新 inode 文件 → 必须拒绝。
///
/// 覆盖范围：
/// - ✅ canonical 出来的 path 不变，但 path 对应的 inode 变了（删 + 同名新建）
///   → inode 比对捕获
/// - ❌ 不覆盖：canonical 出来的 path 仍指向旧 inode，但旧 inode 是 symlink、
///   symlink target 被替换为另一文件（攻击者控制 target 内容）。
///   见 docs/OCR-FIX-PLAN-2026-09-21.md follow-up：
///   "Phase 6: symlink target 替换防护"
///
/// Windows 端 TOCTOU 未实现（保守选择：inode re-check 在 #[cfg(unix)] 下，
/// Windows 上 InodeCheckOutcome::Ok 直接 no-op 放行）——见 commit message。
#[cfg(unix)]
#[tokio::test]
async fn canonical_path_replaced_with_new_inode_is_rejected() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.txt");
    std::fs::write(&file, b"original").unwrap();

    // 1. 模拟"canonicalize 与 read 之间，文件被换"
    //    先记录旧 inode（相当于 resolve_with_perm 完成时的 ino）
    let pre_ino = std::fs::metadata(&file).unwrap().ino();

    //    删除原文件
    std::fs::remove_file(&file).unwrap();
    //    同名新建一个**不同 inode**的文件（不同 mtime/dev 都行，关键是 inode）
    std::fs::write(&file, b"replaced").unwrap();
    let new_ino = std::fs::metadata(&file).unwrap().ino();
    assert_ne!(pre_ino, new_ino, "sanity: 新文件 inode 必须与旧不同");

    // 2. 调用 read_with_ino_check：应返回 Replaced，且**不返回任何内容**
    let (outcome, is_binary, content, _truncated) =
        read_with_ino_check(&file, Some(pre_ino), READ_MAX_BYTES)
            .await
            .unwrap();
    assert_eq!(
        outcome,
        InodeCheckOutcome::Replaced,
        "inode 变了必须返回 Rejected"
    );
    // 顺手验证二进制判定也不丢（这里是文本文件，应为 false）
    assert!(!is_binary, "文本文件的 is_binary 应为 false");
    assert!(
        content.is_empty(),
        "inode 不一致时不得返回内容（内容必须来自核对过的那个 fd）"
    );
}

/// 【 TOCTOU】读到的内容必须来自**核对过 inode 的那个 fd**。
///
/// 原实现分两步：inode 复核时打开的 fd 在闭包内 drop，随后按路径**重新
/// open** 读内容——两步之间文件被换掉，读回来的是新文件的内容。合并成
/// 一次 open 后，比对与读取在同一个 fd 上完成。
#[cfg(unix)]
#[tokio::test]
async fn replaced_file_content_is_never_returned() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.txt");
    std::fs::write(&file, b"original").unwrap();
    let pre_ino = std::fs::metadata(&file).unwrap().ino();

    // 换掉文件：删 + 同名新建（inode 变了，内容也变了）
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, b"replaced-by-attacker").unwrap();

    let (outcome, _is_binary, content, _t) =
        read_with_ino_check(&file, Some(pre_ino), READ_MAX_BYTES)
            .await
            .unwrap();
    assert_eq!(outcome, InodeCheckOutcome::Replaced);
    assert!(
        !content.windows(7).any(|w| w == b"replaced"),
        "被换掉的文件的内容不得被读出来：{content:?}"
    );
}

/// inode 一致时，内容照常读出（合并 fd 后不能把正常读取也弄坏）。
#[cfg(unix)]
#[tokio::test]
async fn matching_inode_returns_the_file_content() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.txt");
    std::fs::write(&file, b"hello\nworld\n").unwrap();
    let pre_ino = std::fs::metadata(&file).unwrap().ino();
    let (outcome, is_binary, content, truncated) =
        read_with_ino_check(&file, Some(pre_ino), READ_MAX_BYTES)
            .await
            .unwrap();
    assert_eq!(outcome, InodeCheckOutcome::Ok);
    assert!(!is_binary);
    assert!(!truncated);
    assert_eq!(content, b"hello\nworld\n");
}

// ── grep_files 支持单文件 ──

/// 路径类型 → 搜法：文件就该只搜它，不该回一句「不是目录」。
///
/// 回归锁：模型为找技能正文的章节标题去 grep 一个溢出指针文件 → 参数闸拒绝
/// （Denied）→ 技能判死 → 整个回合结束（实测 2026-10-26）。
#[test]
fn grep_target_splits_dir_and_single_file() {
    let p = PathBuf::from("/tmp/x/doc.md");
    assert!(matches!(grep_target(Some(true), &p), Some(GrepTarget::Dir)));
    match grep_target(Some(false), &p) {
        Some(GrepTarget::File(f)) => assert_eq!(f, p),
        other => panic!("文件路径应走单文件分支：{other:?}"),
    }
    // 探测失败 → None（调用方报错重试，不猜）
    assert!(grep_target(None, &p).is_none());
}

/// 单文件匹配：glob 过滤 / 逐行匹配 / 命中行渲染，与目录 walk 同一条判定。
#[test]
fn grep_single_file_matches_and_respects_glob() {
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("doc.md");
    std::fs::write(
        &doc,
        "# 标题\n## 工作流程（七步法）\n第一步：需求澄清\n第二步：内容研究\n",
    )
    .unwrap();
    let re = regex::Regex::new("^## ").unwrap();

    let mut hits = Vec::new();
    let mut found = 0usize;
    let mut skipped = Vec::new();
    let mut skipped_total = 0usize;
    grep_file_into(
        &doc,
        &re,
        "",
        50,
        0,
        &mut hits,
        &mut found,
        &mut skipped,
        &mut skipped_total,
    );
    assert_eq!(found, 1, "只应命中「## 工作流程」那一行：{hits:?}");
    assert!(hits[0].contains("工作流程"), "命中行要带内容：{hits:?}");

    // glob 不匹配 → 跳过，命中数 0
    let mut hits2 = Vec::new();
    let mut found2 = 0usize;
    let mut skipped2 = Vec::new();
    let mut skipped_total2 = 0usize;
    grep_file_into(
        &doc,
        &re,
        "*.py",
        50,
        0,
        &mut hits2,
        &mut found2,
        &mut skipped2,
        &mut skipped_total2,
    );
    assert_eq!(found2, 0, "glob 不该匹配时命中数为 0：{hits2:?}");
}
