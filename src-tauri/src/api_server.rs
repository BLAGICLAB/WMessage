//! 服务生命周期：HTTP server 启动 / 端口绑定 / EventHub（SSE 中枢）
//!
//! 仅监听 127.0.0.1 的本地微服务（外部机器人接口）。
//! 完整模块组：
//! - `api_server`   : 此文件 — 服务生命周期 / 端口绑定 / EventHub
//! - `api_auth`     : token 管理 / 鉴权 / 开关持久化
//! - `api_handlers` : endpoint handlers + tauri commands
//! - `api`          : 数据层 `TaskStore` + `MemStore` + `TauriStore`
//!
//! 安全约束：
//! - 只绑定回环地址（127.0.0.1），不监听 0.0.0.0 / 公网
//! - 端口冲突由调用方捕获，回 500；调用方（`api_handlers::api_start`）据此回 `HttpStartFailed`

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tiny_http::Server;

use crate::api::TaskStore;
use crate::audit::AuditLevel;
use crate::db;

/// SSE 事件重放环形缓冲条数（断线重放窗口）
const EVENT_HISTORY: usize = 1000;

/// Tauri 托管的 API 状态（`Mutex<Option<RunningApi>>`）
#[derive(Default)]
pub struct ApiState(pub Mutex<Option<RunningApi>>);

/// 运行中的 HTTP 服务：shutdown flag + 后台 recv 线程
pub struct RunningApi {
    pub shutdown: Arc<AtomicBool>,
    pub handle: Option<std::thread::JoinHandle<()>>,
}

/// SSE 事件中枢：客户端列表 + 自增事件 id + 历史环形缓冲（断线重放）
pub struct EventHub {
    pub clients: Mutex<Vec<Sender<Vec<u8>>>>,
    pub next_id: AtomicU64,
    pub history: Mutex<VecDeque<(u64, String)>>,
}

impl EventHub {
    /// 新建中枢
    pub fn new() -> Arc<Self> {
        Arc::new(EventHub {
            clients: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(0),
            history: Mutex::new(VecDeque::new()),
        })
    }

    /// 当前事件 id（给 SSE 连接首事件用，客户端据此决定下次 `since` 起点）
    pub fn last_id(&self) -> u64 {
        self.next_id.load(Ordering::SeqCst)
    }

    /// 编号、入历史、广播给所有在线客户端
    pub fn broadcast(&self, event: serde_json::Value) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let msg = format!("id: {id}\ndata: {event}\n\n");
        if let Ok(mut h) = self.history.lock() {
            h.push_back((id, msg.clone()));
            while h.len() > EVENT_HISTORY {
                h.pop_front();
            }
        }
        if let Ok(mut clients) = self.clients.lock() {
            clients.retain(|tx| tx.send(msg.as_bytes().to_vec()).is_ok());
        }
    }

    /// 重放 id > since 的历史事件（断线补齐）
    pub fn replay(&self, since: u64) -> Vec<String> {
        match self.history.lock() {
            Ok(h) => h
                .iter()
                .filter(|(id, _)| *id > since)
                .map(|(_, m)| m.clone())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// 启动 HTTP 服务（绑定 127.0.0.1:port）。
///
/// `emit_fn`：任务变更后回调（生产环境=给主窗口发 tasks-updated，看板自动刷新）。
/// `log_path`：访问/变更日志文件（`None`=不记）。
/// `on_error`：服务器异常回调（生产走 `audit_event!` 写 bot.log，测试 `None` 即可）。
///
/// 启动失败返回 `Err`，调用方据此向用户回 `HttpStartFailed`。
pub fn start_api(
    port: u16,
    token: String,
    store: Arc<dyn TaskStore>,
    emit_fn: Option<Box<dyn Fn(&db::Task) + Send + Sync>>,
    log_path: Option<PathBuf>,
    on_error: Option<Box<dyn Fn(AuditLevel, &str, &str) + Send + Sync>>,
) -> Result<RunningApi, String> {
    // 安全红线：只绑回环地址，绝不 0.0.0.0
    let server = Server::http(("127.0.0.1", port))
        .map_err(|e| format!("HTTP 服务启动失败（端口 {port}）：{e}"))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let sd = shutdown.clone();
    let hub = EventHub::new();
    let tk = token.clone();
    let handle = std::thread::spawn(move || loop {
        if sd.load(Ordering::SeqCst) {
            break;
        }
        match server.recv_timeout(Duration::from_millis(400)) {
            Ok(Some(req)) => crate::api_handlers::handle_request(
                req,
                &tk,
                &store,
                &hub,
                &emit_fn,
                &log_path,
            ),
            Ok(None) => {}
            Err(e) => {
                if let Some(log) = &on_error {
                    log(AuditLevel::Error, "api.recv_error", &e.to_string());
                }
                break;
            }
        }
    });
    Ok(RunningApi {
        shutdown,
        handle: Some(handle),
    })
}