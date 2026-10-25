//! 失败原因回归锁：脚本里的失败输出必须走 stderr——打到 stdout 再非零退出，
//! Rust 侧只看得见空 stderr，错误退化成无因的「提取失败：」套娃前缀。

/// 针线拼接：字面量整串写会在脚本文件里自命中
#[test]
fn every_script_error_exit_prints_to_stderr() {
    let needle = concat!("sys.", "exit(1)");
    for src in [
        include_str!("../scripts/extract.py"),
        include_str!("../scripts/harvest_proto.py"),
        include_str!("../scripts/make_docx.py"),
        include_str!("../scripts/make_docx_revisions.py"),
        include_str!("../scripts/make_pdf.py"),
        include_str!("../scripts/make_pptx.py"),
        include_str!("../scripts/make_xlsx.py"),
        include_str!("../scripts/pdf_render.py"),
    ] {
        for line in src.lines().filter(|l| l.contains(needle)) {
            assert!(
                line.contains("file=sys.stderr"),
                "脚本失败输出未走 stderr：{line}"
            );
        }
    }
}
