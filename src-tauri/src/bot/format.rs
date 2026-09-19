//! Bot 输出层的小工具：列名/状态到中文标签的映射，供 tools.rs 摘要使用。

/// 将任务列 ID 翻译为中文标签，用于工具结果摘要。
/// 任何不在 `todo` / `doing` / `done` 的值都按「待办」兜底（与历史行为一致）。
///
/// 列 ID 字面量与前端 `src/types.ts` 的 ColumnId 保持一致；中文 label
/// 与前端 `src/format.ts` 后续若引入同名函数时对齐。
pub(crate) fn column_label(column: &str) -> &'static str {
    match column {
        "doing" => "进行中",
        "done" => "已完成",
        _ => "待办",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_label_known() {
        assert_eq!(column_label("todo"), "待办");
        assert_eq!(column_label("doing"), "进行中");
        assert_eq!(column_label("done"), "已完成");
    }

    #[test]
    fn column_label_unknown_falls_back() {
        assert_eq!(column_label("archived"), "待办");
        assert_eq!(column_label(""), "待办");
    }
}