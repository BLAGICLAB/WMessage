//! Bot 输出层的小工具：列名/状态到中文标签的映射，供 tools.rs 摘要使用。

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_known() {
        assert_eq!(status_label(TaskStatus::Todo), "待办");
        assert_eq!(status_label(TaskStatus::Doing), "进行中");
        assert_eq!(status_label(TaskStatus::Done), "已完成");
    }
}
