// 测试：图片附件纯函数 + 主编编排纯函数

use super::*;

#[test]
fn attach_image_from_block() {
    let dir = std::env::temp_dir().join(format!("wm_img_{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("a.png");
    std::fs::write(&png, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]).unwrap(); // PNG 魔数
    let content = format!("[附件文件]\n- {}\n\n提取图片里的文字", png.display());
    // 白名单根目录传入（测试用临时目录充当白名单根）
    let (v, skipped) = attach_images_in(std::slice::from_ref(&dir), &content);
    assert_eq!(skipped, 0);
    let arr = v.as_array().expect("应返回多模态数组");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[1]["type"], "image_url");
    let url = arr[1]["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:image/png;base64,"), "url: {url}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn attach_image_outside_whitelist_skipped() {
    // 白名单外的图片路径被跳过（防 [附件文件] 块污染外泄）
    let dir = std::env::temp_dir().join(format!("wm_img_{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("a.png");
    std::fs::write(&png, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]).unwrap();
    let content = format!("[附件文件]\n- {}\n\n看看", png.display());
    let other_root = std::env::temp_dir().join("wm_img_other_root");
    let (v, skipped) = attach_images_in(&[other_root], &content);
    assert_eq!(skipped, 1, "白名单外图片应被跳过");
    assert!(v.is_string(), "全部跳过时退回纯文本");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn non_image_returns_plain_text() {
    let (v, _) = attach_images_in(&[], "普通消息 [附件文件]\n- /tmp/x.docx\n\n润色一下");
    assert!(v.is_string(), "无图片时应返回纯文本字符串");
}

#[test]
fn missing_image_file_skipped() {
    let (v, _) = attach_images_in(
        &[std::path::PathBuf::from("/tmp")],
        "[附件文件]\n- /tmp/not_exists_xyz.png\n\n看看",
    );
    assert!(v.is_string(), "图片不存在时退回纯文本");
}

#[test]
fn oversized_image_never_read_into_memory() {
    // 超限大文件（几 GB 全景图、改名 png 的视频）在读之前就该被 metadata 拒掉，
    // 不能整文件进堆（单条消息最多 4 张 × 最后 3 条消息，峰值 4×3×文件大小）。
    let dir = std::env::temp_dir().join(format!("wm_img_big_{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let big = dir.join("huge.png");
    let f = std::fs::File::create(&big).unwrap();
    f.set_len(super::MAX_IMAGE_BYTES as u64 + 1).unwrap();
    drop(f);
    let content = format!("[附件文件]\n- {}\n\n看看", big.display());
    let (v, skipped) = attach_images_in(std::slice::from_ref(&dir), &content);
    assert_eq!(skipped, 0, "超限不算白名单违规，只是不附图");
    assert!(v.is_string(), "超限图片不进多模态数组");
    // 工具回传侧同口径：超限 → None
    assert!(
        super::media::image_part_from_file(&big).is_none(),
        "工具图片回传对超限文件同样不读"
    );
    std::fs::remove_dir_all(&dir).ok();
}
