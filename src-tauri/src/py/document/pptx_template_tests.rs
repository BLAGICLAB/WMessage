use super::*;

#[test]
fn resolve_prefers_explicit_then_default() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.pptx"), b"x").unwrap();
    std::fs::write(dir.join("b.pptx"), b"x").unwrap();
    std::fs::write(default_marker_path(dir), "b").unwrap();

    assert!(pptx_template_resolve_in(dir, Some("a")).is_some());
    // 显式名不存在 → None（不回退默认，调用方拿内置母版顶）
    assert!(pptx_template_resolve_in(dir, Some("nope")).is_none());
    assert_eq!(
        pptx_template_resolve_in(dir, None).unwrap(),
        dir.join("b.pptx").to_string_lossy().to_string()
    );
    std::fs::remove_file(dir.join("b.pptx")).unwrap();
    assert!(pptx_template_resolve_in(dir, None).is_none());
}

#[test]
fn builtin_master_materializes_idempotent_and_self_heals() {
    let tmp = tempfile::tempdir().unwrap();
    // 空资产不合法：内嵌母版必须是 zip（pptx = zip 容器，PK 魔数）
    assert!(BUILTIN_PPTX_MASTER.len() > 1000);
    assert_eq!(&BUILTIN_PPTX_MASTER[..2], b"PK");

    let first = builtin_pptx_master_in(tmp.path()).unwrap();
    let written = std::fs::read(&first).unwrap();
    assert_eq!(written, BUILTIN_PPTX_MASTER);
    // 幂等：二次调用字节不变不再写（mtime 不变）
    let meta1 = std::fs::metadata(&first).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let again = builtin_pptx_master_in(tmp.path()).unwrap();
    assert_eq!(again, first);
    let meta2 = std::fs::metadata(&again).unwrap().modified().unwrap();
    assert_eq!(meta1, meta2, "字节一致时不得重写");
    // 自愈：缓存损坏后重写
    std::fs::write(&first, b"corrupted").unwrap();
    builtin_pptx_master_in(tmp.path()).unwrap();
    assert_eq!(std::fs::read(&first).unwrap(), BUILTIN_PPTX_MASTER);
}

#[test]
fn script_fills_placeholders_without_theme_code() {
    // 回归锁：v2 脚本走母版占位符填充，主题/自定义配色代码不得回流
    assert!(MAKE_PPTX_SCRIPT.contains("Presentation(p['template'])"));
    assert!(!MAKE_PPTX_SCRIPT.contains("THEMES"));
    assert!(!MAKE_PPTX_SCRIPT.contains("customColors"));
    assert!(
        MAKE_PPTX_SCRIPT.contains("MAX_BULLETS = 5"),
        "密度拆页闸必须在"
    );
}
