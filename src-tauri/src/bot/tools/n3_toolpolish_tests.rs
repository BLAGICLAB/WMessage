// N3-TOOLPOLISH：六项升级纯函数测试

use super::*;

// ── N3-1：fetch_url offset 切片 ──

#[test]
fn fetch_slice_short_text_unchanged_no_header() {
    // 短文本 + 无 offset：与旧版输出零差异（不加 [位置] 行）
    let out = slice_fetch_output("你好世界", 0);
    assert_eq!(out, "你好世界");
}

#[test]
fn fetch_slice_truncates_with_offset_hint() {
    let text = "A".repeat(35_000);
    let out = slice_fetch_output(&text, 0);
    assert!(out.contains("已截断"), "{out}");
    assert!(
        out.contains("offset=30000"),
        "应给续读点：{}",
        &out[out.len() - 120..]
    );
    assert_eq!(out.matches('A').count(), 30000, "正文截 30000");
}

#[test]
fn fetch_slice_offset_reads_next_page_with_header() {
    let text: String = (0..35_000)
        .map(|i| char::from_u32(0x4e00 + (i % 500) as u32).unwrap())
        .collect();
    let out = slice_fetch_output(&text, 30000);
    assert!(out.starts_with("[位置] 30000–35000 / 共 35000 字符\n"));
    // 第二页读完 → 不再带截断提示
    assert!(!out.contains("已截断"), "35K 总量第二页应读完");
}

#[test]
fn fetch_slice_offset_beyond_total_gives_hint() {
    let out = slice_fetch_output("短文本", 100);
    assert_eq!(out, "offset 100 已超出网页总长 3 字符，没有更多内容");
}

// ── N3-2：扫描 PDF 判定 ──

#[test]
fn pdf_empty_detection_ignores_page_markers() {
    assert!(pdf_text_looks_empty("=== 第1页 ===\n\n=== 第2页 ===\n  \n"));
    assert!(pdf_text_looks_empty(""), "空输出本身就是无文本层");
    assert!(!pdf_text_looks_empty("=== 第1页 ===\n合同正文"));
    // 标记只在行首才算（行中出现的相似文本是正文）
    assert!(!pdf_text_looks_empty("正文里引用了 === 第3页 === 字样"));
}

// ── N3-4：create_word images 清洗 ──

#[test]
fn filter_images_in_keeps_only_gen_dir_files() {
    let dir = std::env::temp_dir().join(format!("wm-img-t-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let inside = dir.join("pic.png");
    std::fs::write(&inside, b"png").unwrap();
    let outside = std::env::temp_dir().join("outside.png");
    let _ = std::fs::remove_file(&outside);
    std::fs::write(&outside, b"png").unwrap();
    let gen_canon = std::fs::canonicalize(&dir).ok();
    let (out, dropped) = filter_images_in(
        gen_canon.as_deref(),
        vec![
            inside.display().to_string(),
            outside.display().to_string(),
            "/nope/x.png".into(),
        ],
    );
    assert_eq!(out.len(), 1, "仅 gen 内图片放行：{out:?}");
    assert_eq!(dropped, 2);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_file(&outside).ok();
}
