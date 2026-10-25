//! 日志截断：截断后保尾、低于上限不动、append 续写不留空洞。

use super::*;

// ── 日志截断 ──

#[test]
fn truncate_keeps_tail_and_drops_head() {
    let path = std::env::temp_dir().join(format!("wm-svc-trunc-{}", uuid::Uuid::new_v4()));
    // 前 100 字节是头，后 100 字节是尾
    let mut content = vec![b'H'; 100];
    content.extend(vec![b'T'; 100]);
    std::fs::write(&path, &content).expect("写临时文件");

    let dropped = truncate_output_file(&path, 128).expect("截断");
    assert!(dropped > 0);
    let after = std::fs::read(&path).expect("读回");
    assert!(
        after.len() <= 128,
        "截断后大小必须 <= cap，实际 {}",
        after.len()
    );
    // 只保留末尾 cap/2 字节：原文件尾部 64 字节全是 'T'，头部 100 个 'H' 全丢
    assert_eq!(after.len(), 64, "截断后应剩 cap/2 字节");
    assert!(after.iter().all(|b| *b == b'T'), "留下的必须全是尾部字节");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn truncate_is_noop_under_cap() {
    let path = std::env::temp_dir().join(format!("wm-svc-trunc-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"short").expect("写临时文件");
    let dropped = truncate_output_file(&path, 1024).expect("截断");
    assert_eq!(dropped, 0);
    assert_eq!(std::fs::read(&path).expect("读回"), b"short");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn truncate_rewrite_does_not_create_holes() {
    // 关键：宿主写回用 append 模式 → 截断后续写落在新末尾，不产生 NUL 空洞
    let path = std::env::temp_dir().join(format!("wm-svc-trunc-{}", uuid::Uuid::new_v4()));
    let mut content = vec![b'H'; 100];
    content.extend(vec![b'T'; 100]);
    std::fs::write(&path, &content).expect("写临时文件");

    truncate_output_file(&path, 128).expect("截断");
    // 模拟子进程续写（append）
    {
        let mut f = open_append(&path).expect("append 打开");
        use std::io::Write;
        f.write_all(b"NEXT").expect("续写");
    }
    let after = std::fs::read(&path).expect("读回");
    assert!(!after.contains(&0), "文件里不该有 NUL 空洞");
    assert_eq!(after.len(), 64 + 4, "截断后 64 字节 + 续写 4 字节");
    assert!(
        after.ends_with(b"NEXT"),
        "续写必须落在文件末尾，实际尾部 {:?}",
        String::from_utf8_lossy(&after[after.len().saturating_sub(8)..])
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn tail_lines_returns_last_n_lines() {
    let path = std::env::temp_dir().join(format!("wm-svc-tail-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, "l1\nl2\nl3\nl4\nl5\n").expect("写临时文件");
    let t = tail_lines(&path, 2);
    assert_eq!(t, vec!["l4".to_string(), "l5".to_string()]);
    let _ = std::fs::remove_file(&path);
}
