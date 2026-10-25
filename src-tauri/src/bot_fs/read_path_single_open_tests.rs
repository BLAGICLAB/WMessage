//! U7 接线守卫：`tool_read_text_file` 的读路径**只开一次文件**。
//!
//! inode 复核若与实际读取分属两次 open，中间就是 TOCTOU 窗口——复核的是旧
//! fd，交给用户的是新文件的内容。行为级测试只能覆盖「已经换掉」这一种情形，
//! 验不到「函数体里又多了一次 open」这种接线退化，所以这里扫源码。
//!
//! 用花括号配平截函数体（而不是 `split_once("#[cfg(test)]")` 切生产段）：
//! 本文件的测试模块在文件头部也有一个，位置不在生产代码之后，切不出干净的
//! 「生产段」。

/// 从锚点起按花括号配平截出函数体。
fn fn_body<'a>(src: &'a str, anchor: &str) -> &'a str {
    let start = src
        .find(anchor)
        .unwrap_or_else(|| panic!("锚点应存在：{anchor}"));
    let open = start
        + src[start..]
            .find('{')
            .unwrap_or_else(|| panic!("锚点后应有函数体：{anchor}"));
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open..=open + i];
                }
            }
            _ => {}
        }
    }
    panic!("函数体未闭合：{anchor}");
}

#[test]
fn read_text_file_reads_through_one_verified_fd() {
    let src = include_str!("read.rs");
    let body = fn_body(src, "pub async fn tool_read_text_file(");
    assert_eq!(
        body.matches("read_with_ino_check(").count(),
        1,
        "读路径必须且只能调一次 read_with_ino_check（复核与读取合并在同一个 fd 上）"
    );
    assert!(
        !body.contains("read_capped_file"),
        "读路径不得再按路径二次 open 读内容（那正是 TOCTOU 窗口）"
    );
    assert!(
        !body.contains("File::open") && !body.contains("fs::read"),
        "读路径不得自行 open/read 文件——内容必须来自已复核的那个 fd"
    );
}

/// 守卫的守卫：抽取器本身得能截对函数体，否则上面的断言恒真。
#[test]
fn fn_body_extractor_ends_at_the_right_brace() {
    let src = include_str!("read.rs");
    let body = fn_body(src, "pub async fn tool_read_text_file(");
    assert!(body.starts_with('{') && body.ends_with('}'), "{body}");
    assert!(
        !body.contains("pub async fn tool_grep_files("),
        "函数体不得越过 tool_read_text_file 的边界"
    );
    assert!(
        body.contains("read_with_ino_check("),
        "函数体应包含读路径调用"
    );
}
