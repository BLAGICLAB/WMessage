// 文件变更证据（Agent 透明化设计 §4.2）

use super::*;

#[test]
fn diff_modify_counts_and_unified_text() {
    let before = "a\nb\nc\n";
    let after = "a\nX\nc\nd\n";
    let (diff, added, deleted, truncated) = build_unified_diff(before, after, "/x/f.txt");
    assert_eq!(added, 2, "插入 X 行 + d 行");
    assert_eq!(deleted, 1, "删除 b 行");
    assert!(!truncated);
    let d = diff.unwrap();
    assert!(
        d.starts_with("--- /x/f.txt"),
        "unified header 用目标路径：{d}"
    );
    assert!(d.contains("+X"), "{d}");
    assert!(d.contains("-b"), "{d}");
}

/// hunk 头的行号语义是前端算「两段之间省略了多少行未改动」的依据：
/// 计数含上下文行（git 语义），故「下一段起始行 − 本段起始行 − 本段计数」
/// 就是中间被省略的旧行数。依赖升级改了 similar 的输出格式时这里会先炸——
/// 前端那套算式依赖的就是这个约定。
#[test]
fn diff_hunk_headers_follow_git_line_number_semantics() {
    let before: String = (1..=12).map(|i| format!("line{i}\n")).collect();
    let after = before
        .replace("line2\n", "LINE2\n")
        .replace("line10\n", "LINE10\n");
    let (diff, _, _, _) = build_unified_diff(&before, &after, "/x.txt");
    let d = diff.unwrap();
    assert!(d.contains("@@ -1,5 +1,5 @@"), "hunk 头格式：\n{d}");
    assert!(d.contains("@@ -7,6 +7,6 @@"), "hunk 头格式：\n{d}");
    // 第 1 段覆盖旧行 1..5（1/3/4/5 上下文 + 第 2 行删除），第 2 段从旧行 7 起
    // → 中间省略旧行 6
    assert!(
        !d.contains(" line6\n"),
        "旧行 6 不该出现（两段之间的省略区）：\n{d}"
    );
}

#[test]
fn diff_create_all_insertions() {
    let (diff, added, deleted, truncated) = build_unified_diff("", "l1\nl2\n", "/x/new.txt");
    assert_eq!(added, 2);
    assert_eq!(deleted, 0);
    assert!(!truncated);
    assert!(diff.unwrap().contains("+l1"));
}

#[test]
fn diff_identical_content_yields_no_diff() {
    let (diff, added, deleted, truncated) = build_unified_diff("same\n", "same\n", "/x");
    assert!(diff.is_none(), "零变更不产 diff（write 同内容覆盖场景）");
    assert_eq!((added, deleted, truncated), (0, 0, false));
}

#[test]
fn diff_truncates_text_but_keeps_counts() {
    // MAX_DIFF_LINES=2000：2500 行新文件 → 文本截断置位，±计数不截（计数是事实）
    let after: String = (0..2500).map(|i| format!("line{i}\n")).collect();
    let (diff, added, deleted, truncated) = build_unified_diff("", &after, "/x/big.txt");
    assert!(truncated);
    assert_eq!(added, 2500);
    assert_eq!(deleted, 0);
    let d = diff.unwrap();
    assert!(d.contains("已截断"), "截断标记必须可见：{d}");
    assert_eq!(
        d.lines().count(),
        crate::db::MAX_DIFF_LINES + 1,
        "正文 2000 行 + 截断提示行"
    );
}

#[test]
fn diff_handles_crlf_content() {
    let before = "a\r\nb\r\n";
    let after = "a\r\nB\r\n";
    let (diff, added, deleted, _) = build_unified_diff(before, after, "/x/crlf.txt");
    assert_eq!((added, deleted), (1, 1));
    let d = diff.unwrap();
    assert!(d.contains("-b"), "{d:?}");
}

#[test]
fn receipt_create_has_no_before_evidence() {
    let r = build_file_change_receipt("/x/new.txt", "create", None, "hello\n", 0);
    assert_eq!(r.kind, "create");
    assert_eq!(r.path, "/x/new.txt");
    assert_eq!(r.added, 1);
    assert_eq!(r.deleted, 0);
    assert_eq!(
        r.replacements, 0,
        "create 路径无 find_iter，replacements 恒 0"
    );
    assert!(r.before_sha.is_none(), "create 无 before 证据");
    assert!(r.before_ref.is_none(), "快照由调用方回填，builder 恒 None");
    assert_eq!(r.after_sha.as_deref(), Some(sha256_hex("hello\n").as_str()));
    assert!(r.diff.unwrap().contains("+hello"));
}

#[test]
fn receipt_modify_carries_before_sha_and_diff() {
    let r = build_file_change_receipt("/x/f.py", "modify", Some("old\n"), "new\n", 1);
    assert_eq!(r.kind, "modify");
    assert_eq!(r.added, 1);
    assert_eq!(r.deleted, 1);
    assert_eq!(r.replacements, 1);
    assert_eq!(r.before_sha.as_deref(), Some(sha256_hex("old\n").as_str()));
    assert_eq!(r.after_sha.as_deref(), Some(sha256_hex("new\n").as_str()));
    assert!(r.diff.unwrap().contains("-old"));
}

#[test]
fn tool_result_carries_file_changes_and_defaults_empty() {
    let r = ToolResult::ok("x", Vec::new())
        .with_file_change(build_file_change_receipt("/a", "create", None, "b\n", 0));
    assert_eq!(r.file_changes.len(), 1);
    assert_eq!(r.file_changes[0].path, "/a");
    // 其余构造器默认空——既有内置工具零感知（加字段不改语义的兼容锁）
    assert!(ToolResult::ok("o", Vec::new()).file_changes.is_empty());
    assert!(ToolResult::warn("w", Vec::new()).file_changes.is_empty());
    assert!(ToolResult::error("e", Vec::new()).file_changes.is_empty());
}

/// sha256 已知向量（回滚证据链的算法稳定性锁——换库/换编码当场炸）
#[test]
fn sha256_hex_known_vector() {
    assert_eq!(
        sha256_hex("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn generated_name_excludes_minified_and_lockfiles() {
    // minified 产物与锁文件（大小写不敏感）
    assert!(is_generated_name("app.min.js"));
    assert!(is_generated_name("App.MIN.CSS"));
    assert!(is_generated_name("bundle.min.mjs"));
    assert!(is_generated_name("sourcemap.map"));
    assert!(is_generated_name("package-lock.json"));
    assert!(is_generated_name("Cargo.lock"));
    // 正常源码/文档不得误伤
    assert!(!is_generated_name("main.js"));
    assert!(!is_generated_name("sitemap.ts"));
    assert!(!is_generated_name("lock.txt"));
    assert!(!is_generated_name("readme.md"));
}

#[test]
fn skip_note_hidden_when_nothing_skipped() {
    assert!(format_skip_note(&[], 0).is_none());
}

#[test]
fn skip_note_lists_names_under_cap() {
    let skipped = vec!["a.log".to_string(), "b.bundle.js".to_string()];
    let note = format_skip_note(&skipped, 2).expect("有跳过必有提示");
    assert!(note.contains("2 个大文件"), "{note}");
    assert!(note.contains("a.log、b.bundle.js"), "{note}");
    assert!(note.contains("read_text_file"), "{note}");
    assert!(
        !note.contains("等共"),
        "未超名单容量不得出现「等共」字样: {note}"
    );
}

#[test]
fn skip_note_marks_total_beyond_name_cap() {
    let skipped = vec!["a".to_string(); 5];
    let note = format_skip_note(&skipped, 9).expect("有跳过必有提示");
    assert!(note.contains("等共 9 个"), "{note}");
}
