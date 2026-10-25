use super::*;

#[test]
fn bounded_read_roundtrip_small_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("w.json");
    std::fs::write(&p, r#"[{"kind":"note"}]"#).unwrap();
    let s = read_import_bounded(p.to_str().unwrap(), 1024).unwrap();
    assert_eq!(s, r#"[{"kind":"note"}]"#);
}

/// 超限必须按大小闸拒绝，而不是被 Take 截断后误报成 UTF-8 编码错误
#[test]
fn bounded_read_rejects_over_limit_instead_of_truncating() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("big.json");
    std::fs::write(&p, "a".repeat(9)).unwrap();
    let err = read_import_bounded(p.to_str().unwrap(), 8).unwrap_err();
    assert!(
        format!("{err:?}").contains("大小上限"),
        "应是大小闸拒绝：{err:?}"
    );
}
