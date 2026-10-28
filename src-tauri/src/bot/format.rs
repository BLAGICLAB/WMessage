//! Bot 输出层的小工具：列名/状态到中文标签的映射、字节口径文本截断。

use crate::db::TaskStatus;

/// 将任务状态 enum 翻译为中文标签，用于工具结果摘要。
/// exhaustive match：TaskStatus 只有 Todo / Doing / Done 三个 variant，不需要兜底。
///
/// label 字符串与前端 `src/format.ts` 后续若引入同名函数时对齐。
pub(crate) fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Doing => "进行中",
        TaskStatus::Done => "已完成",
        TaskStatus::Todo => "待办",
    }
}

/// 头部可安全切片的下标：取前 n_bytes 字节，切点回退到 UTF-8 字符边界。
///
/// 工具输出体积上限统一按字节计（模型循环的 `effective_output_cap` 生效点）：
/// `String::len()` 是字节、`chars().count()` 是字符，拿字符数去卡一条字节预算，
/// 中文内容会漏掉 3 倍。切点落在多字节字符中间时必须回退——切出半个 UTF-8 字符
/// 会让回灌文本变成乱码，模型读到的就是坏数据。
pub(crate) fn byte_head_end(text: &str, n_bytes: usize) -> usize {
    let mut end = n_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// 尾部可安全切片的下标：从末尾往前取 n_bytes 字节，切点回退到字符边界。
/// 回退方向是往后（取更少字节），宁可少给也不能切出半个字符。
pub(crate) fn byte_tail_start(text: &str, n_bytes: usize) -> usize {
    let mut start = text.len().saturating_sub(n_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

/// 按字节截断字符串（不切破 UTF-8 字符）。不超 max_bytes 原样返回。
pub(crate) fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    text[..byte_head_end(text, max_bytes)].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_known() {
        assert_eq!(status_label(TaskStatus::Todo), "待办");
        assert_eq!(status_label(TaskStatus::Doing), "进行中");
        assert_eq!(status_label(TaskStatus::Done), "已完成");
    }

    #[test]
    fn truncate_bytes_counts_bytes_not_chars() {
        // 中文 3 字节/字：50 字 = 150 字节，cap 30 字节只能留 10 个字
        let text = "中".repeat(50);
        let out = truncate_bytes(&text, 30);
        assert_eq!(out, "中".repeat(10));
        assert!(out.len() <= 30, "截断后字节数必须不超 cap");
    }

    #[test]
    fn truncate_bytes_never_splits_a_multibyte_char() {
        // cap 落在汉字中间（3 字节字符的第 2 字节）→ 回退到字符边界，不得出现半个字
        let text = "中".repeat(10);
        for cap in 1..=30 {
            let out = truncate_bytes(&text, cap);
            assert!(
                out.chars().all(|c| c == '中'),
                "cap={cap} 切出了半个字符：{out:?}"
            );
            assert!(out.len() <= cap, "cap={cap} 截断后字节数超了");
        }
    }

    #[test]
    fn truncate_bytes_under_cap_unchanged() {
        assert_eq!(truncate_bytes("hello", 30_000), "hello");
        assert_eq!(truncate_bytes(&"x".repeat(100), 100), "x".repeat(100));
    }

    #[test]
    fn tail_start_never_splits_a_multibyte_char() {
        let text = "中".repeat(10); // 30 字节
        for n in 1..=30 {
            let start = byte_tail_start(&text, n);
            assert!(text.is_char_boundary(start), "n={n} 切点落在字符中间");
            assert!(text.len() - start <= n + 2, "n={n} 取多了字节");
        }
    }
}
