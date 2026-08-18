//! 对外任务对象 `TaskOut`
//!
//! A5：抽离自 `api_handlers.rs` —— `api.rs`（数据层）需要 `TaskOut` 序列化 SSE 事件，
//! 但数据层不应反向依赖 `api_handlers`。抽到这里供 `api` 与 `api_handlers` 共享。
//!
//! 字段：`db::Task` flatten + `status`（即看板列，与 `t.column` 同值，前端用 status 渲染）。

use serde::Serialize;

use crate::db;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskOut {
    #[serde(flatten)]
    pub inner: db::Task,
    pub status: String,
}

impl TaskOut {
    pub fn from_task(t: &db::Task) -> Self {
        TaskOut {
            inner: t.clone(),
            status: t.column.clone(),
        }
    }
}