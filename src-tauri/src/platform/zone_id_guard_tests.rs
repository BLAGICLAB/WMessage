//! 「来自互联网」标记收口的源码级锁：复制 / 搬运路径必须走收口原语。
//!
//! 背景：2026-10-30 实机又挨了一次 360 主动防御——Word 删除 Office 文档缓存里
//! 的 `~WRDxxxx.docx` 时被杀软按风险程序连坐拦下（风险文件记的是 wmessage.exe）。
//! 根因不是 app 自产的文档（新建文件天生不带标记），而是**把用户已有文件
//! 复制 / 搬运出去**的那几条路径：复制走 Win32 `CopyFileEx`，标记流跟着一起复制。
//!
//! 这类口子漏一条就静默复发，所以用源码级锁盯住：
//! - 搬运模块里不许出现裸 `fs::copy`，必须走 `zone_id::copy_file`（它按
//!   「文档/图片」口径收口；可执行文件等保留标记，那是实质安全信号）；
//! - 搬运（改名）路径必须走 `zone_id::strip_after_move`——改名是同一个文件，
//!   标记原样跟着走，落点却已经换成 app 交付 / 归档的位置。

/// 复制收口：模块里必须调 `zone_id::copy_file`，且不许有裸 `fs::copy`。
fn assert_copy_closed_out(label: &str, src: &str) {
    assert!(
        src.contains("zone_id::copy_file"),
        "{label} 的复制没走 zone_id::copy_file——复制会连带复制「来自互联网」标记，\
         副本落在用户会双击打开的位置，就会触发 Office 受保护视图 → 写文档缓存 → 杀软连坐"
    );
    assert!(
        !src.contains("fs::copy("),
        "{label} 出现了裸 fs::copy——复制必须走 zone_id::copy_file（它按「文档/图片」口径收口）"
    );
}

/// 搬运收口：模块里必须调 `zone_id::strip_after_move`。
fn assert_move_closed_out(label: &str, src: &str) {
    assert!(
        src.contains("zone_id::strip_after_move"),
        "{label} 的搬运没收口——改名是同一个文件（标记原样跟着走），\
         落点却换成了 app 交付 / 归档的位置"
    );
}

#[test]
fn copy_paths_go_through_zone_id_closeout() {
    assert_copy_closed_out("bot_manage_file.rs", include_str!("../bot_manage_file.rs"));
    assert_copy_closed_out("migration/ops.rs", include_str!("../migration/ops.rs"));
    assert_copy_closed_out(
        "bot_skills/manage.rs",
        include_str!("../bot_skills/manage.rs"),
    );
}

#[test]
fn move_paths_go_through_zone_id_closeout() {
    assert_move_closed_out("bot_manage_file.rs", include_str!("../bot_manage_file.rs"));
    assert_move_closed_out("migration/ops.rs", include_str!("../migration/ops.rs"));
}

/// 修订版 Word 的产物收口必须排在**失败返回之前**。
///
/// 两个引擎都是「先复制原文再就地修订」，失败时那份带标记的副本已经躺在产物
/// 目录里了——收口排在失败返回之后，这条路径就漏了。
#[test]
fn revision_product_strips_mark_before_failure_return() {
    let src = include_str!("../py/document.rs");
    let strip = src
        .find("strip_download_mark(&app, Path::new(&out), \"doc_make_word_revisions\")")
        .expect("修订版 Word 的产物收口丢了");
    let fail = src
        .find("return Err(script_fail_err(\"生成修订版 Word 失败\"")
        .expect("修订版 Word 的失败返回丢了");
    assert!(
        strip < fail,
        "收口必须排在失败返回之前——失败时产物目录里躺着带「来自互联网」标记的副本"
    );
}
