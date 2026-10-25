//! 产物收口回归锁：每个交付给用户的文档产物都要剥掉「来自互联网」标记。
//!
//! 标记留着，Office 打开它会走受保护视图、把文件复制进文档缓存
//! （`INetCache\Content.Word\~WRDxxxx.docx`），杀软随即把这次「写文档」
//! 按风险程序连坐拦掉——2026-10-23 实机挨的就是这个。
//!
//! 新增文档产物时必须一并接上收口，这个锁不让它漏。

#[test]
fn every_doc_product_strips_download_mark() {
    let src = include_str!("../document.rs");
    // 针线拼接写法：include_str 含测试自身源码，整串字面量会自命中
    let call = concat!("strip_download_", "mark(&app, Path::new(&out), ");
    for name in [
        "doc_make_word",
        "doc_make_word_revisions",
        "doc_make_excel",
        "doc_make_pdf",
        "doc_make_ppt",
    ] {
        assert!(
            src.contains(&format!("{call}\"{name}\")")),
            "{name} 产物没接「剥来自互联网标记」收口"
        );
    }
    // 模板上传那条落点是 dest 不是 out，单独锁
    assert!(
        src.contains(concat!(
            "strip_download_",
            "mark(&app, &dest, \"word_template_import\")"
        )),
        "word_template_import 没接「剥来自互联网标记」收口"
    );
}
