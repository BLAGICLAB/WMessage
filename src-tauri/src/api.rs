//! 数据访问抽象（数据层）
//!
//! 本地 HTTP API 模块组入口（仅数据层）。完整模块组：
//! - `api_server`   : 服务生命周期 / 端口绑定 / EventHub
//! - `api_auth`     : token 管理 / 鉴权 / 开关持久化
//! - `api_handlers` : endpoint handlers + tauri commands
//! - `api`（此处）  : `TaskStore` trait + `MemStore` + `TauriStore`
//!
//! 数据层只暴露 trait + 实现，handler / server 通过 `Arc<dyn TaskStore>` 注入，
//! 方便单测用 `MemStore`、生产用 `TauriStore`（走 SQLite）。

use std::sync::Mutex;
use tauri::AppHandle;

use crate::db;

/// HTTP 服务端口（仅回环；与 `api_server` / `api_handlers` 共享）
pub const API_PORT: u16 = 4763;

/// 任务存储抽象：抽象层便于单测，`MemStore` 是内存实现（测试用），
/// `TauriStore` 是生产实现（走 SQLite，与看板同一份数据）。
pub trait TaskStore: Send + Sync {
    fn load(&self) -> Result<Vec<db::Task>, String>;
    fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String>;
}

/// 内存存储实现：测试 / 单测用，加锁模拟并发写。
#[allow(dead_code)] // 仅在测试模块（api_handlers::tests）构造
pub struct MemStore {
    pub tasks: Mutex<Vec<db::Task>>,
}

impl TaskStore for MemStore {
    fn load(&self) -> Result<Vec<db::Task>, String> {
        Ok(self.tasks.lock().unwrap().clone())
    }
    fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String> {
        let mut g = self.tasks.lock().unwrap();
        for t in tasks {
            if let Some(i) = g.iter().position(|x| x.id == t.id) {
                g[i] = t;
            } else {
                g.push(t);
            }
        }
        Ok(())
    }
}

/// 生产实现：走 SQLite（与看板同一份数据）
pub struct TauriStore {
    pub app: AppHandle,
}

impl TaskStore for TauriStore {
    fn load(&self) -> Result<Vec<db::Task>, String> {
        db::db_load(self.app.clone()).map_err(|e| e.to_string())
    }
    fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String> {
        db::db_upsert(self.app.clone(), tasks).map_err(|e| e.to_string())
    }
}