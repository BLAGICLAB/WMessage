//! 数据访问抽象（数据层）
//!
//! 本地 HTTP API 模块组入口（仅数据层）。完整模块组：
//! - `api_server`   : 服务生命周期 / 端口绑定
//! - `api_auth`     : token 管理 / 鉴权 / 开关持久化
//! - `api_handlers` : endpoint handlers + tauri commands
//! - `api`（此处）  : `TaskStore` trait + `MemStore` + `TauriStore`
//!
//! 数据层只暴露 trait + 实现，handler / server 通过 `Arc<dyn TaskStore>` 注入，
//! 方便单测用 `MemStore`、生产用 `TauriStore`（走 SQLite）。
//!
//! A5：EventHub 从 API server 移到 store 层。SSE 中枢成为 store 的一部分，
//! 写路径走 `notify_change()` 广播，与 API 生命周期解耦。
//! 测试构造 `MemStore` 时仍可自传 hub（与生产解耦不强迫）。

use std::sync::{Arc, Mutex};
use tauri::AppHandle;

use crate::api_server::EventHub;
use crate::db;

/// HTTP 服务端口（仅回环；与 `api_server` / `api_handlers` 共享）
pub const API_PORT: u16 = 4763;

/// 任务存储抽象：抽象层便于单测，`MemStore` 是内存实现（测试用），
/// `TauriStore` 是生产实现（走 SQLite，与看板同一份数据）。
///
/// `event_hub()`：返回 store 内嵌的 SSE 中枢引用（SSE 客户端注册 / 广播均通过 store）。
/// `notify_change()`：写操作完成后调用，store 内部广播 SSE（保证前端看板实时更新）。
pub trait TaskStore: Send + Sync {
    fn load(&self) -> Result<Vec<db::Task>, String>;
    fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String>;
    fn event_hub(&self) -> &Arc<EventHub>;
    fn notify_change(&self, op: &str, task: &db::Task);
}

/// 内存存储实现：测试 / 单测用，加锁模拟并发写。
///
/// `hub` 字段：可外部传入（测试复用固定 hub）或构造时 `EventHub::new()`。
#[allow(dead_code)] // 仅在测试模块（api_handlers::tests）构造
pub struct MemStore {
    pub tasks: Mutex<Vec<db::Task>>,
    pub hub: Arc<EventHub>,
}

impl TaskStore for MemStore {
    fn load(&self) -> Result<Vec<db::Task>, String> {
        Ok(self.tasks.lock().unwrap().clone())
    }
    fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String> {
        let mut g = self.tasks.lock().unwrap();
        for t in tasks {
            // 2026-09-04 审计 P2-4：与 db.rs upsert 的 T1-1 基线比对对齐——原先完全忽略
            // expected_updated_at，API 层 409 冲突路径在 MemStore 下永远不可测（测试基建空洞）
            if let Some(expected) = t.expected_updated_at {
                let cur = g.iter().find(|x| x.id == t.id);
                let conflict = if expected == db::BASELINE_NULL_ROW {
                    // 行存在性基线：行仍在且 updated_at 仍为 NULL 才放行
                    !matches!(cur, Some(x) if x.updated_at.is_none())
                } else {
                    cur.and_then(|x| x.updated_at) != Some(expected)
                };
                if conflict {
                    return Err(format!(
                        "{}：任务 {} 读快照后已被其他写者修改或删除，本次整行写回被拒",
                        db::CONFLICT_ERR_PREFIX,
                        t.id
                    ));
                }
            }
            if let Some(i) = g.iter().position(|x| x.id == t.id) {
                g[i] = t;
            } else {
                g.push(t);
            }
        }
        Ok(())
    }
    fn event_hub(&self) -> &Arc<EventHub> {
        &self.hub
    }
    fn notify_change(&self, op: &str, task: &db::Task) {
        let event = serde_json::json!({
            "type": "tasks-changed",
            "op": op,
            "task": crate::task_out::TaskOut::from_task(task),
        });
        self.hub.broadcast(event);
    }
}

/// 生产实现：走 SQLite（与看板同一份数据）
///
/// `hub` 与 store 同生命周期：API 启动时构造 TauriStore 时新建，
/// API 停止时 store 引用释放，hub 自然回收（SyncSender 持有者清零后 channel drop）。
pub struct TauriStore {
    pub app: AppHandle,
    pub hub: Arc<EventHub>,
}

impl TaskStore for TauriStore {
    fn load(&self) -> Result<Vec<db::Task>, String> {
        // B3: db_load/db_upsert 改 async 了；TaskStore trait 仍是 sync（handler 在 per-request
        // std::thread 里跑，不在 tokio runtime 上 → block_on 不会死锁）。
        tauri::async_runtime::block_on(async { db::db_load(self.app.clone()).await })
            .map_err(|e| e.to_string())
    }
    fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String> {
        tauri::async_runtime::block_on(async { db::db_upsert(self.app.clone(), tasks).await })
            .map_err(|e| e.to_string())
    }
    fn event_hub(&self) -> &Arc<EventHub> {
        &self.hub
    }
    fn notify_change(&self, op: &str, task: &db::Task) {
        let event = serde_json::json!({
            "type": "tasks-changed",
            "op": op,
            "task": crate::task_out::TaskOut::from_task(task),
        });
        self.hub.broadcast(event);
    }
}