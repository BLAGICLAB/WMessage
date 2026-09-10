//! 任务变更来源标记（编译期枚举，替代字符串约定）。
//!
//! `tasks-updated` 事件的 `source` 字段标识本次变更由谁写入：
//! 主窗口收到后据此决定是否回写 SQLite（Bot/Api/Migration 已由后端线程落盘，只合并 UI；
//! Widget/未标记 = 挂件上报，主窗口统一落盘）。
//! 协议值保持原字符串不变；前端对应定义在 `src/lib/mutationOrigin.ts`，改动需两侧同步。

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MutationOrigin {
    Main,
    Widget,
    Bot,
    Api,
    Migration,
}

impl MutationOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Widget => "widget",
            Self::Bot => "bot",
            Self::Api => "api",
            Self::Migration => "migration",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_serde_lowercase() {
        // 协议值锁死：serde 序列化结果必须与 as_str 一致（前端按字符串比较）
        for (origin, s) in [
            (MutationOrigin::Main, "main"),
            (MutationOrigin::Widget, "widget"),
            (MutationOrigin::Bot, "bot"),
            (MutationOrigin::Api, "api"),
            (MutationOrigin::Migration, "migration"),
        ] {
            assert_eq!(origin.as_str(), s);
            assert_eq!(serde_json::to_value(origin).unwrap(), serde_json::json!(s));
        }
    }
}
