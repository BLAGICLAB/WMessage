use super::*;
use std::fs;

/// 原子写 happy path——内容完整落盘，tmp 不残留；覆盖已有文件也正常。
#[test]
fn atomic_write_success_and_overwrite() {
    let dir = std::env::temp_dir().join(format!("wm-aw-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("export.json");

    atomic_write(&p, "{\"v\":1}").unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "{\"v\":1}");
    assert!(!dir.join("export.json.tmp").exists(), "tmp 不应残留");

    // 覆盖已有目标（导出到已存在的文件）
    atomic_write(&p, "{\"v\":2}").unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "{\"v\":2}");
    fs::remove_dir_all(&dir).ok();
}

/// tmp 写入失败（目标目录只读）→ 目标文件保持原状，不被半截覆盖。
#[test]
fn atomic_write_failure_preserves_target() {
    let dir = std::env::temp_dir().join(format!("wm-aw-ro-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("export.json");
    fs::write(&p, "original-complete").unwrap();

    // 目录只读 → 无法创建 tmp → 写失败
    let mut perms = fs::metadata(&dir).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&dir, perms).unwrap();

    let r = atomic_write(&p, "partial-json-that-must-not-land");

    // 恢复可写以便清理与读回校验
    let mut perms = fs::metadata(&dir).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&dir, perms).unwrap();

    assert!(r.is_err(), "只读目录下 tmp 写入应失败");
    assert_eq!(
        fs::read_to_string(&p).unwrap(),
        "original-complete",
        "失败时目标文件必须保持原状"
    );
    fs::remove_dir_all(&dir).ok();
}

// ── check_export_path（tasks_export / tasks_import 共用的 .json 扩展名闸门） ──

#[test]
fn check_export_path_accepts_json_case_insensitive() {
    assert!(tasks::check_export_path("/tmp/a.json").is_ok());
    assert!(tasks::check_export_path("/tmp/a.JSON").is_ok());
}

#[test]
fn check_export_path_rejects_non_json_extension() {
    let err = tasks::check_export_path("/tmp/a.txt").expect_err("非 .json 应拒绝");
    assert!(err.to_string().contains(".json"), "错误应提到 .json：{err}");
}

#[test]
fn check_export_path_rejects_missing_extension() {
    assert!(tasks::check_export_path("/tmp/a").is_err());
    assert!(tasks::check_export_path("/tmp/dir/").is_err());
}

// ── W11：check_export_path 加固（..词法/父目录/符号链） ──

#[test]
fn check_export_path_rejects_traversal_and_missing_parent() {
    // 词法 .. 组件：任何位置都拒
    assert!(tasks::check_export_path("/tmp/../etc/a.json").is_err());
    assert!(tasks::check_export_path("a/../../b.json").is_err());
    // 父目录不存在
    assert!(tasks::check_export_path("/nonexistent-dir-xyz/a.json").is_err());
    // 空路径
    assert!(tasks::check_export_path("   ").is_err());
}

#[cfg(unix)]
#[test]
fn check_export_path_rejects_symlink_target_and_accepts_new_file() {
    let dir = std::env::temp_dir().join(format!("w11-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // 已存在的符号链目标：拒（写穿透 fail-closed）
    let secret = dir.join("secret.json");
    std::fs::write(&secret, b"{}").unwrap();
    let link = dir.join("link.json");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let err = tasks::check_export_path(link.to_str().unwrap()).expect_err("符号链应拒绝");
    assert!(err.to_string().contains("符号链"), "{err}");
    // 父目录经软链解析到真实目录：放行（软链目录是合法的存放位置）
    let real_dir = std::env::temp_dir().join(format!("w11-real-{}", std::process::id()));
    std::fs::create_dir_all(&real_dir).unwrap();
    let link_dir = dir.join("link-dir");
    std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();
    assert!(
        tasks::check_export_path(link_dir.join("out.json").to_str().unwrap()).is_ok(),
        "软链父目录应放行"
    );
    // 不存在的新文件 + 真实父目录：放行（save dialog 主路径不受影响）
    assert!(tasks::check_export_path(dir.join("new-out.json").to_str().unwrap()).is_ok());
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&real_dir).ok();
}
