//! 产物落盘路径：`gen_out_path_in` 的命名与不覆盖保证。
//!
//! 卡 ACC-T-12 判据 3/4 的自动化面：「同名加 `(n)`，永不覆盖」此前只有源码
//! 锚点、没有用例——命名逻辑一旦回退，用户的旧产物会被静默覆盖。

use super::gen_out_path_in;
use std::fs;

/// 产物文件名字： basename + 扩展名
fn name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

#[test]
fn first_generation_takes_the_plain_name() {
    let dir = tempfile::tempdir().unwrap();
    let out = gen_out_path_in(dir.path(), Some("周报"), "docx").unwrap();
    assert_eq!(name_of(&out), "周报.docx");
    assert!(!std::path::Path::new(&out).exists(), "只算路径，不落文件");
}

#[test]
fn same_name_gets_incremented_suffix_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    // 序号靠「盘上已存在」推进：每轮都要把上一轮的文件真的落盘，
    // 否则第二次仍然会拿到 (1)（函数看的是 exists()，不是内存计数）
    let first = gen_out_path_in(dir.path(), Some("周报"), "docx").unwrap();
    assert_eq!(name_of(&first), "周报.docx");
    fs::write(&first, "第一份").unwrap();

    let second = gen_out_path_in(dir.path(), Some("周报"), "docx").unwrap();
    assert_eq!(name_of(&second), "周报 (1).docx");
    assert_ne!(second, first);
    fs::write(&second, "第二份").unwrap();

    let third = gen_out_path_in(dir.path(), Some("周报"), "docx").unwrap();
    assert_eq!(name_of(&third), "周报 (2).docx");
    assert_ne!(third, second);
}

#[test]
fn existing_files_keep_their_content() {
    let dir = tempfile::tempdir().unwrap();
    let first = gen_out_path_in(dir.path(), Some("周报"), "docx").unwrap();
    fs::write(&first, "第一份内容").unwrap();

    let second = gen_out_path_in(dir.path(), Some("周报"), "docx").unwrap();
    fs::write(&second, "第二份内容").unwrap();

    // 覆盖即缺陷：第一份的内容必须原样还在
    assert_eq!(fs::read_to_string(&first).unwrap(), "第一份内容");
    assert_eq!(fs::read_to_string(&second).unwrap(), "第二份内容");
}

#[test]
fn filename_with_path_is_reduced_to_basename() {
    let dir = tempfile::tempdir().unwrap();
    let out = gen_out_path_in(dir.path(), Some("/tmp/别的目录/周报"), "docx").unwrap();
    assert_eq!(name_of(&out), "周报.docx");
}

#[test]
fn known_extension_is_not_doubled() {
    let dir = tempfile::tempdir().unwrap();
    // 文件名自带扩展名时不该出「周报.docx.docx」
    let out = gen_out_path_in(dir.path(), Some("周报.docx"), "docx").unwrap();
    assert_eq!(name_of(&out), "周报.docx");
}

#[test]
fn missing_directory_is_created_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("AI_Gen_Files");
    assert!(!nested.exists());
    let out = gen_out_path_in(&nested, Some("周报"), "docx").unwrap();
    assert!(nested.exists(), "产物目录解析即建");
    assert_eq!(name_of(&out), "周报.docx");
}
