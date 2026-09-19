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
            status: t.column.as_str().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task() -> db::Task {
        db::Task {
            id: "t1".into(),
            title: "测试任务".into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column: crate::db::TaskStatus::Doing,
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: Some(1.0),
            updated_at: Some(1000),
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            expected_updated_at: None,
        }
    }

    /// TaskOut 线缆形状契约锁——HTTP API/SSE 前端依赖
    /// ① flatten（task 字段平铺，不嵌套 inner）② camelCase ③ status 与 column 同值。
    /// serde 属性被破坏时普通测试抓不到，只有前端运行时炸。
    #[test]
    fn task_out_wire_shape_locked() {
        let v = serde_json::to_value(TaskOut::from_task(&sample_task())).unwrap();
        let obj = v.as_object().expect("TaskOut 必须序列化为平铺对象");
        assert!(obj.contains_key("id"), "flatten 失效：task 字段未平铺");
        assert!(!obj.contains_key("inner"), "flatten 失效：出现嵌套 inner");
        assert!(
            obj.contains_key("botAssigned") || !obj.contains_key("bot_assigned"),
            "camelCase 失效"
        );
        assert_eq!(obj["status"], "doing");
        assert_eq!(obj["status"], obj["column"], "status 必须与 column 同值");
    }
}
