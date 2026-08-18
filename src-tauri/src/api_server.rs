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
use std::sync::mpsc::SyncSender;
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

/// SSE 事件中枢：客户端列表（bounded 256） + 自增事件 id + 历史环形缓冲（断线重放）
pub struct EventHub {
    pub clients: Mutex<Vec<SyncSender<Vec<u8>>>>, // bounded SyncSender 端；client 端持 Rx
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
        // A2: sync_channel(256) + try_send — 队列满时 try_send 立即返回 Err，广播不阻塞
        if let Ok(mut clients) = self.clients.lock() {
            let mut i = 0;
            while i < clients.len() {
                // try_send: bounded 队列满时 Err 表示 client 积压过深，跳过并移除
                if clients[i].try_send(msg.as_bytes().to_vec()).is_err() {
                    clients.remove(i);
                } else {
                    i += 1;
                }
            }
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
    // A5: hub 不再此处构造；EventHub 现属于 store（store.event_hub() 访问）
    let tk = token.clone();
    // emit_fn: Box → Arc 包装，使每个 per-request worker 能拿到独立 clone
    let emit_fn: Option<Arc<dyn Fn(&db::Task) + Send + Sync>> =
        emit_fn.map(|b| -> Arc<dyn Fn(&db::Task) + Send + Sync> { b.into() });
    let handle = std::thread::spawn(move || loop {
        if sd.load(Ordering::SeqCst) {
            break;
        }
        match server.recv_timeout(Duration::from_millis(400)) {
            Ok(Some(req)) => {
                // A1 + A4 组合：每个请求独立 worker 线程 + catch_unwind +
                //              主线程  15s 超时 (防止 slowloris 永久卡死服务)
                let req_url = req.url().to_string();
                let (done_tx, done_rx) = std::sync::mpsc::channel();
                let emit_fn_w = emit_fn.clone();
                let log_path_w = log_path.clone();
                let tk_w = tk.clone();
                let store_w = store.clone();
                std::thread::spawn(move || {
                    let catch_result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| {
                            crate::api_handlers::handle_request(
                                req,
                                &tk_w,
                                &store_w,
                                &emit_fn_w,
                                &log_path_w,
                            );
                        }),
                    );
                    if let Err(payload) = catch_result {
                        let msg = panic_message(payload);
                        // worker 内调 audit_event! 不方便；通过通道传上去
                        let _ = done_tx.send(Err(msg));
                    } else {
                        let _ = done_tx.send(Ok(()));
                    }
                });
                match done_rx.recv_timeout(Duration::from_secs(15)) {
                    Ok(Ok(())) => {}
                    Ok(Err(msg)) => {
                        if let Some(log) = &on_error {
                            log(
                                AuditLevel::Error,
                                "api.handler_panic",
                                &format!("{req_url}: {msg}"),
                            );
                        }
                    }
                    Err(_) => {
                        // worker 仍在读 body/等客户端，bottleneck 不在本服务
                        // 客户端断开 / body 超过 15s 都会让 worker 自行退出
                        if let Some(log) = &on_error {
                            log(
                                AuditLevel::Error,
                                "api.handler_timeout",
                                &format!("{req_url}: 超时 15s，worker 续跑直到客户端断开"),
                            );
                        }
                    }
                }
            }
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

/// 从 catch_unwind payload 提取 panic 信息（处理 &str / String / 其他三种情况）
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}