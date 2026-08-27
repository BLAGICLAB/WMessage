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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tiny_http::{Response, Server, StatusCode};

use crate::api::TaskStore;
use crate::audit::AuditLevel;
use crate::db;

/// SSE 事件重放环形缓冲条数（断线重放窗口）
const EVENT_HISTORY: usize = 1000;

/// 并发 worker 上限（A6：thread-per-request 无上限时，慢连接会无限堆积 OS 线程）
const MAX_WORKERS: usize = 64;

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
    // P2-3：通道载荷带事件 id——断线重放与在线推送可能交叠（重放快照期间广播的新事件
    // 既进 history 又进在线队列），writer 端靠 id 去重（id <= 已发最大 id 则跳过）
    // 2026-08-28 批次4审计 P1-3：条目携带 writer 存活令牌（Weak，writer 线程持 Arc）——
    // 原先死连接的 sender 只在下次广播 try_send 失败时才移除，安静期内尸体占满
    // MAX_SSE_CLIENTS 名额导致新连接被 503；sse_connect 注册前先按令牌收割尸体。
    pub clients: Mutex<Vec<(SyncSender<(u64, Vec<u8>)>, std::sync::Weak<()>)>>,
    pub next_id: AtomicU64,
    pub history: Mutex<VecDeque<(u64, String)>>,
    /// 事件 id 持久化路径（跨重启保持单调；None=仅内存，测试用）
    id_path: Option<PathBuf>,
}

impl EventHub {
    /// 新建中枢
    #[allow(dead_code)] // 生产走 persisted()；new() 仅测试（MemStore / 单测）构造用
    pub fn new() -> Arc<Self> {
        Arc::new(EventHub {
            clients: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(0),
            history: Mutex::new(VecDeque::new()),
            id_path: None,
        })
    }

    /// 带 id 持久化的中枢（A6）：启动时从文件恢复上次 id，保证跨重启单调递增。
    /// 否则重启后 id 从 0 重计，客户端按 Last-Event-ID 去重会静默丢弃全部新事件。
    pub fn persisted(path: PathBuf) -> Arc<Self> {
        let start = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        Arc::new(EventHub {
            clients: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(start),
            history: Mutex::new(VecDeque::new()),
            id_path: Some(path),
        })
    }

    /// 当前事件 id（给 SSE 连接首事件用，客户端据此决定下次 `since` 起点）
    pub fn last_id(&self) -> u64 {
        self.next_id.load(Ordering::SeqCst)
    }

    /// 编号、入历史、广播给所有在线客户端
    pub fn broadcast(&self, event: serde_json::Value) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        // A6: 每次广播落盘当前 id（事件频率为人级，开销可忽略），重启后接续递增
        if let Some(p) = &self.id_path {
            let _ = std::fs::write(p, id.to_string());
        }
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
                if clients[i].0.try_send((id, msg.as_bytes().to_vec())).is_err() {
                    clients.remove(i);
                } else {
                    i += 1;
                }
            }
        }
    }

    /// 重放 id > since 的历史事件（断线补齐）
    /// P2-3：返回 (id, msg)，writer 端据此与在线推送去重
    pub fn replay(&self, since: u64) -> Vec<(u64, String)> {
        match self.history.lock() {
            Ok(h) => h
                .iter()
                .filter(|(id, _)| *id > since)
                .map(|(id, m)| (*id, m.clone()))
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
    // A6: 在飞 worker 计数（配合 MAX_WORKERS 上限，防慢连接线程堆积）
    let active = Arc::new(AtomicUsize::new(0));
    let handle = std::thread::spawn(move || loop {
        if sd.load(Ordering::SeqCst) {
            break;
        }
        match server.recv_timeout(Duration::from_millis(400)) {
            Ok(Some(req)) => {
                // A6: worker 数上限 —— 超限直接 503，不再无上限 spawn 线程
                if active.load(Ordering::SeqCst) >= MAX_WORKERS {
                    let _ = req.respond(
                        Response::from_data(br#"{"error":"server busy"}"#.to_vec())
                            .with_status_code(StatusCode(503)),
                    );
                    continue;
                }
                active.fetch_add(1, Ordering::SeqCst);
                let active_w = active.clone();
                // A1 + A4 组合：每个请求独立 worker 线程 + catch_unwind +
                //              主线程 15s 超时（只作用于 handler 执行阶段——
                //              2026-08-28 批次4审计 P1-1：tiny_http 在 recv 内部顺序读完
                //              header 才产出 Request，header 阶段的 slowloris 滴注
                //              到不了这里；accept 级防护需换 HTTP 栈，列为已知残留）
                let req_url = req.url().to_string();
                let (done_tx, done_rx) = std::sync::mpsc::channel();
                let emit_fn_w = emit_fn.clone();
                let log_path_w = log_path.clone();
                let tk_w = tk.clone();
                let store_w = store.clone();
                std::thread::spawn(move || {
                    // 配额归还守卫：无论正常完成 / panic / 超时后续跑，退出即归还
                    let _guard = ActiveGuard(active_w);
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

/// worker 退出时归还并发配额（正常完成 / panic / 超时后续跑结束都会触发）
struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A6：事件 id 跨"重启"（drop 后重建 hub）保持单调递增
    #[test]
    fn event_hub_id_persists_across_restart() {
        let path = std::env::temp_dir().join(format!(
            "wmessage-test-event-id-{}.txt",
            uuid::Uuid::new_v4()
        ));
        let hub = EventHub::persisted(path.clone());
        hub.broadcast(serde_json::json!({"type":"tasks-changed","op":"created"}));
        hub.broadcast(serde_json::json!({"type":"tasks-changed","op":"updated"}));
        assert_eq!(hub.last_id(), 2);
        drop(hub);

        // 模拟重启：从文件恢复 id，继续递增而非归零
        let hub2 = EventHub::persisted(path.clone());
        assert_eq!(hub2.last_id(), 2);
        hub2.broadcast(serde_json::json!({"type":"tasks-changed","op":"deleted"}));
        assert_eq!(hub2.last_id(), 3);
        let _ = std::fs::remove_file(&path);
    }

    /// 无持久化路径的 hub（测试/内存用）不受影响
    #[test]
    fn event_hub_new_starts_from_zero() {
        let hub = EventHub::new();
        hub.broadcast(serde_json::json!({"type":"tasks-changed","op":"created"}));
        assert_eq!(hub.last_id(), 1);
    }

    /// P2-3：since=5 断线重放——4 个 id<5 + id=5 本身都不重放，只推 id>5 的 6 条；
    /// 且每条带回事件 id，供 writer 端与在线推送去重（重放窗口内广播的事件既进
    /// history 又进在线队列，不带 id 就会重复推给客户端）
    #[test]
    fn replay_since_returns_only_newer_events_with_ids() {
        let hub = EventHub::new();
        for i in 1..=11u64 {
            hub.broadcast(serde_json::json!({"type":"tasks-changed","seq":i}));
        }
        // id 1-4（<5）+ id 5（=since）跳过，id 6-11 共 6 条重放
        let replayed = hub.replay(5);
        assert_eq!(replayed.len(), 6, "只应重放 id>5 的事件: {replayed:?}");
        for (idx, (id, msg)) in replayed.iter().enumerate() {
            let expect_id = 6 + idx as u64;
            assert_eq!(*id, expect_id, "重放事件 id 必须单调且连续");
            assert!(
                msg.starts_with(&format!("id: {expect_id}\n")),
                "SSE 帧头 id 与返回 id 必须一致: {msg:?}"
            );
        }
        // since=0（全新客户端）→ 全量 11 条；since=last_id → 空
        assert_eq!(hub.replay(0).len(), 11);
        assert!(hub.replay(11).is_empty());
    }
}