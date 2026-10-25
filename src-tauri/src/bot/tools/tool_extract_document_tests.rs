use super::*;

#[test]
fn format_extract_output_short_text_returns_full_text_no_truncation_suffix() {
    let text = "短文本".repeat(100); // 100 个汉字 = 300 chars，远低于默认 limit
    let out = format_extract_output("/tmp/sample.md", &text, 0, EXTRACT_DEFAULT_LIMIT);
    assert!(
        out.contains(&text),
        "短文本应原样保留：\n--out--\n{out}\n--text--\n{text}"
    );
    assert!(
        !out.contains("已截断"),
        "短文本不应出现截断提示，实际输出：\n{out}"
    );
    assert!(
        out.starts_with("[文档路径] /tmp/sample.md\n[位置] 0–300 / 共 300 字符\n[文档内容]\n"),
        "头部应带位置行，实际：\n{out}"
    );
}

#[test]
fn format_extract_output_long_text_truncates_with_suffix() {
    // 35000 个 'A'，超默认 30000
    let text = "A".repeat(35000);
    let out = format_extract_output("/tmp/big.md", &text, 0, EXTRACT_DEFAULT_LIMIT);
    assert!(
        out.contains("已截断"),
        "长文本必须出现截断提示，实际输出末尾：\n{}",
        &out[out.len().saturating_sub(200)..]
    );
    // 输出含有的 'A' 数量应 == 30000（截断后）
    let a_count = out.matches('A').count();
    assert_eq!(
        a_count, 30000,
        "长文本截断后应剩 30000 个 'A'，实际 {a_count}"
    );
    // 续读提示给出下一页起点
    assert!(
        out.contains("offset=30000"),
        "截断提示应给续读 offset：\n{out}"
    );
}

#[test]
fn format_extract_output_offset_reads_next_page() {
    let text = "A".repeat(35000);
    let out = format_extract_output("/tmp/big.md", &text, 30000, EXTRACT_DEFAULT_LIMIT);
    assert!(
        out.contains("[位置] 30000–35000 / 共 35000 字符"),
        "第二页位置行：\n{out}"
    );
    assert_eq!(out.matches('A').count(), 5000, "第二页应只有剩余 5000 字符");
    assert!(!out.contains("已截断"), "读完最后一页不应再有截断提示");
}

#[test]
fn format_extract_output_offset_beyond_total() {
    let text = "短";
    let out = format_extract_output("/tmp/a.md", text, 100, EXTRACT_DEFAULT_LIMIT);
    assert!(
        out.contains("超出文档总长"),
        "offset 越界应明确提示：\n{out}"
    );
}
