// 抑制 unused 警告

use super::*;

#[test]
fn resolve_prefers_explicit_then_default() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.docx"), b"x").unwrap();
    std::fs::write(dir.join("b.docx"), b"x").unwrap();
    std::fs::write(default_marker_path(dir), "b").unwrap();

    // 显式名命中（存在才返回）
    assert!(word_template_resolve_in(dir, Some("a")).is_some());
    // 显式名不存在 → None（不回退默认，用户点名了就不能拿别的顶）
    assert!(word_template_resolve_in(dir, Some("nope")).is_none());
    // 未指定 → 默认
    assert_eq!(
        word_template_resolve_in(dir, None).unwrap(),
        dir.join("b.docx").to_string_lossy().to_string()
    );
    // 默认文件被删 → None
    std::fs::remove_file(dir.join("b.docx")).unwrap();
    assert!(word_template_resolve_in(dir, None).is_none());
}

#[test]
fn resolve_without_marker_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(word_template_resolve_in(tmp.path(), None).is_none());
}
