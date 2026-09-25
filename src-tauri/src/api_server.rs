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
/// 缓冲满后最老事件被挤出——客户端以早于缓冲最老 id 的
/// `?since=` 重连时会缺段（丢失的事件无任何补发通道）。事件频率为人级操作，
/// 1000 条窗口对实际断线重连足够；要彻底覆盖需持久化事件日志，暂不做。
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
    /// 本服务实例实际绑定的 Bearer token。
    /// api_start 的「已有活服务」早退分支据此返回**权威** token：
    /// 调用方 Phase 1 从磁盘读到的 token 与此刻内存服务绑定的可能不同
    /// （中间插入过 api_rotate_token），返回旧值会让调用方拿到对不上活服务的凭证。
    pub token: String,
}

/// SSE 事件中枢：客户端列表（bounded 256） + 自增事件 id + 历史环形缓冲（断线重放）
pub struct EventHub {
    // 通道载荷带事件 id——断线重放与在线推送可能交叠（重放快照期间广播的新事件
    // 既进 history 又进在线队列），writer 端靠 id 去重（id <= 已发最大 id 则跳过）
    // 条目携带 writer 存活令牌（Weak，writer 线程持 Arc）——
    // 死连接的 sender 若只在下次广播 try_send 失败时才移除，安静期内尸体占满
    // MAX_SSE_CLIENTS 名额导致新连接被 503；sse_connect 注册前先按令牌收割尸体。
    pub clients: Mutex<Vec<(SyncSender<(u64, Vec<u8>)>, std::sync::Weak<()>)>>,
    pub next_id: AtomicU64,
    pub history: Mutex<VecDeque<(u64, String)>>,
    /// hub 身份键：构造时从全局计数器取，进程内单调递增。
    /// 用于 SSE writer 分组(api_stop 按 hub_id 通知停止)、api_rotate_token
    /// 识别「是否还是同一个 hub」。旧实现用 `Arc::as_ptr(&hub) as usize`,
    /// 指针可能在 Arc drop 后被复用,造成跨 stop/start 的 writer 误关联。
    /// 注:hub_id **不跨进程** — 进程重启后 EVENT_HUB_COUNTER 从 1 重计,
    /// 不同进程生命周期会复用同一 ID。writers 全在内存、进程退出全部销毁,
    /// 不会跨进程误关联。`api_status` 也不会持久化 hub_id。
    /// 只读访问一律走 `hub_id()` getter（对外不再暴露字段本身）。
    hub_id: u64,
    /// 事件 id 持久化路径（跨重启保持单调；None=仅内存，测试用）
    id_path: Option<PathBuf>,
}

/// hub 身份键单调计数器（进程内；每次 `fetch_add(1)` 返回一个不重复的 u64）。
/// 从 1 起跳,避免与 API_HUB_KEY 的 0 哨兵（= 无 hub 实例）冲突。
/// 不暴露到 sse.rs 是因为只有 EventHub 构造时取 ID,
/// sse_connect 通过 `hub.hub_id()` 读;sse/commands 只用 API_HUB_KEY
/// 跟踪「当前 API 实例」的 hub_id。
static EVENT_HUB_COUNTER: AtomicU64 = AtomicU64::new(1);

impl EventHub {
    /// 新建中枢
    #[allow(dead_code)] // 生产走 persisted()；new() 仅测试（MemStore / 单测）构造用
    pub fn new() -> Arc<Self> {
        Arc::new(EventHub {
            clients: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(0),
            history: Mutex::new(VecDeque::new()),
            hub_id: EVENT_HUB_COUNTER.fetch_add(1, Ordering::SeqCst),
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
            hub_id: EVENT_HUB_COUNTER.fetch_add(1, Ordering::SeqCst),
            id_path: Some(path),
        })
    }

    /// hub 身份键（用于 SSE writer 分组、rotate 识别）
    pub fn hub_id(&self) -> u64 {
        self.hub_id
    }

    /// 当前事件 id（给 SSE 连接首事件用，客户端据此决定下次 `since` 起点）
    pub fn last_id(&self) -> u64 {
        self.next_id.load(Ordering::SeqCst)
    }

    /// 编号、入历史、广播给所有在线客户端
    ///
    /// id 契约（sse.rs 断线重放去重的先决条件）：进程内严格单调递增——
    /// fetch_add 在本函数 clients 锁内执行，id 分配与推送同临界区，
    /// 每个 client 队列的事件序 = id 序；跨重启单调由 persisted() 的
    /// id_path 落盘保证；hub 换代时旧 writer 全停（api_stop），无跨代复用。
    pub fn broadcast(&self, event: serde_json::Value) {
        // 单临界区（OCR C5-AP-04）：fetch_add / 落盘 / history / 推送全在
        // clients 锁内。若 fetch_add 在锁外，并发广播 A(id5)/B(id6) 可 B 先
        // 入队——writer 端 `id <= last_sent` 去重会静默丢迟到的 id5；同理
        // id 落盘在锁外可致先 6 后 5 落盘，重启后 id 回退复用。
        // 锁序：clients→history 是全仓唯一嵌套点（其余站点均单锁），无死锁对。
        // 事件频率为人级，锁内文件 I/O 开销可忽略（落盘移出锁需另加同步
        // 才能保住「落盘序 = id 序」，代价大于收益）。
        // Value 序列化在锁外先做（payload KB 级），锁内只剩拼接。
        let data = event.to_string();
        let mut clients = self.clients.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] api_server::broadcast clients: {e:?}");
            e.into_inner()
        });
        // SeqCst 与 last_id() 的锁外读保持一致（纯锁内序 Relaxed 也够，此处取一致性）
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        // A6: 每次广播落盘当前 id（事件频率为人级，开销可忽略），重启后接续递增
        // 用 atomic_write（tmp+rename）落盘——fs::write 直写崩溃会留半截文件，
        // 重启 id 归 0 → 客户端 Last-Event-ID 去重静默丢全部新事件。
        // 写失败拒推进（拍板 #16=A fail-closed）：归还 id + 本事件不进历史不推送
        // ——「要么持久化要么不推进」，重启后 Last-Event-ID 语义不破。归还后下一次
        // 广播重新 fetch_add 取同一 id 重试写盘（本临界区内单写者，无竞争）。
        if let Some(p) = &self.id_path {
            if let Err(e) = db::atomic_write(p, &id.to_string()) {
                eprintln!("[event_hub] id persistence failed, event dropped (fail-closed): {e}");
                self.next_id.fetch_sub(1, Ordering::SeqCst);
                return;
            }
        }
        let msg = format!("id: {id}\ndata: {data}\n\n");
        if let Ok(mut h) = self.history.lock() {
            h.push_back((id, msg.clone()));
            while h.len() > EVENT_HISTORY {
                h.pop_front();
            }
        }
        // A2: sync_channel(256) + try_send — 队列满时 try_send 立即返回 Err，广播不阻塞
        let mut i = 0;
        while i < clients.len() {
            // try_send: bounded 队列满时 Err 表示 client 积压过深，跳过并移除
            if clients[i]
                .0
                .try_send((id, msg.as_bytes().to_vec()))
                .is_err()
            {
                clients.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// 重放 id > since 的历史事件（断线补齐）
    /// 返回 (id, msg)，writer 端据此与在线推送去重
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
    // A5: hub 由 store 持有（store.event_hub() 访问），不在此处构造
    let tk = token.clone();
    // emit_fn: Box → Arc 包装，使每个 per-request worker 能拿到独立 clone
    let emit_fn: Option<Arc<dyn Fn(&db::Task) + Send + Sync>> =
        emit_fn.map(|b| -> Arc<dyn Fn(&db::Task) + Send + Sync> { b.into() });
    // A6: 在飞 worker 计数（配合 MAX_WORKERS 上限，防慢连接线程堆积）
    let active = Arc::new(AtomicUsize::new(0));
    // on_error 包 Arc 传入每个 worker 自行记录 panic——
    // 若 worker 经 channel 回传、accept 线程同步等结果，任一慢请求期间新请求
    // 会全部排队，api_stop 的 join 也会被拖长；等结果的唯一收益只是把
    // panic 消息带回 accept 线程记日志。
    let on_error: Option<Arc<dyn Fn(AuditLevel, &str, &str) + Send + Sync>> =
        on_error.map(|b| -> Arc<dyn Fn(AuditLevel, &str, &str) + Send + Sync> { b.into() });
    let handle = std::thread::spawn(move || loop {
        if sd.load(Ordering::SeqCst) {
            break;
        }
        match server.recv_timeout(Duration::from_millis(400)) {
            Ok(Some(req)) => {
                // A6: worker 数上限 —— 原子占位（OCR C5-AP-01）：fetch_add 返回值
                // 即占位序号，超限立即归还并 503；不再 load+fetch_add 两步竞态
                // （burst 下两线程可同时观察到 active < MAX 都放行）。
                // （SeqCst 与文件内其余原子一致——纯占位计数用 Relaxed 也够，此处取一致性）
                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                if n > MAX_WORKERS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    let _ = req.respond(
                        Response::from_data(br#"{"error":"server busy"}"#.to_vec())
                            .with_status_code(StatusCode(503)),
                    );
                    continue;
                }
                let active_w = active.clone();
                // A1: 每个请求独立 worker 线程 + catch_unwind（panic 不带垮 accept 循环）。
                // spawn 后 accept 线程立即回到 recv，不等 worker——
                // worker 的 panic 由 worker 自己经 on_error 记录。
                // （tiny_http 在 recv 内部顺序读完
                //  header 才产出 Request，header 阶段的 slowloris 滴注到不了这里；
                //  accept 级防护需换 HTTP 栈，列为已知残留）
                let req_url = req.url().to_string();
                let emit_fn_w = emit_fn.clone();
                let log_path_w = log_path.clone();
                let tk_w = tk.clone();
                let store_w = store.clone();
                let on_error_w = on_error.clone();
                std::thread::spawn(move || {
                    // 配额归还守卫：无论正常完成 / panic，退出即归还
                    let _guard = ActiveGuard(active_w);
                    let catch_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            crate::api_handlers::handle_request(
                                req,
                                &tk_w,
                                &store_w,
                                &emit_fn_w,
                                &log_path_w,
                            );
                        }));
                    if let Err(payload) = catch_result {
                        let msg = crate::audit::panic_message(payload);
                        if let Some(log) = &on_error_w {
                            log(
                                AuditLevel::Error,
                                "api.handler_panic",
                                &format!("{req_url}: {msg}"),
                            );
                        }
                    }
                });
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
        token,
    })
}

/// worker 退出时归还并发配额（正常完成 / panic 都会触发）
struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘写失败 fail-closed（拍板 #16=A）：broadcast 时 id 无法持久化 → 归还 id
    /// （last_id 不变=下次 broadcast 重取同 id）且事件不进历史不推送。
    /// 恢复段用新 hub 验证独立正常路径（同 hub 写盘恢复需真实目录翻转，不模拟）。
    #[test]
    fn broadcast_persist_failure_fails_closed() {
        // id_path 指向目录：atomic_write 的 rename(→目录) 必败（EISDIR）
        let dir = std::env::temp_dir().join(format!(
            "wmessage-test-event-hub-dir-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let hub = EventHub::persisted(dir.clone());
        let (tx, rx) = std::sync::mpsc::sync_channel::<(u64, Vec<u8>)>(256);
        hub.clients
            .lock()
            .unwrap()
            .push((tx, std::sync::Weak::new()));

        hub.broadcast(serde_json::json!({"type":"tasks-changed","op":"created"}));

        // fail-closed：id 未推进、事件未入队
        assert_eq!(hub.last_id(), 0, "写失败 id 必须归还（拒推进）");
        assert!(rx.try_recv().is_err(), "事件不得投递给 client");
        let h = hub.history.lock().unwrap();
        assert!(h.is_empty(), "事件不得进入重放历史");
        drop(h);

        // 恢复路径：可写 id 文件的新 hub 正常广播，id 从 1 接续（归还语义）
        let ok_path = std::env::temp_dir().join(format!(
            "wmessage-test-event-hub-ok-{}.txt",
            uuid::Uuid::new_v4()
        ));
        let hub2 = EventHub::persisted(ok_path.clone());
        hub2.broadcast(serde_json::json!({"type":"tasks-changed","op":"created"}));
        assert_eq!(hub2.last_id(), 1, "恢复后 id 从 1 正常接续");
        assert!(ok_path.exists(), "id 文件已落盘");
        let _ = std::fs::remove_dir_all(&dir);
        // atomic_write rename 失败会残留同级 tmp（既有缺口，另登记）——测试自清理
        if let Some(parent) = dir.parent() {
            for e in std::fs::read_dir(parent).unwrap() {
                let p = e.unwrap().path();
                if p.file_name()
                    .map(|n| n.to_string_lossy().contains(".tmp"))
                    .unwrap_or(false)
                {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        let _ = std::fs::remove_file(&ok_path);
    }

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

    /// accept 级 slowloris 回归（vendor tiny_http 读超时 patch）。
    /// 滴注不完整 header 后静默的连接，服务端在读超时后必须断开它（408 或 EOF），
    /// 且 accept 循环仍能服务后续新请求。
    #[test]
    fn slowloris_silent_connection_dropped_and_server_survives() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        // 进程级读超时临时调小（默认 30s 跑测试太慢）；guard 保证 panic 也恢复
        tiny_http::HTTP_READ_TIMEOUT_MS.store(400, Ordering::Relaxed);
        struct RestoreReadTimeout;
        impl Drop for RestoreReadTimeout {
            fn drop(&mut self) {
                tiny_http::HTTP_READ_TIMEOUT_MS.store(30_000, Ordering::Relaxed);
            }
        }
        let _restore = RestoreReadTimeout;

        // 动态端口：先占 :0 拿空闲端口再释放，避免与其他测试的固定端口（4882x）冲突
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let store: Arc<dyn TaskStore> = Arc::new(crate::api::MemStore {
            tasks: Mutex::new(Vec::new()),
            hub: EventHub::new(),
        });
        let running = start_api(port, "tok".into(), store, None, None, None).unwrap();

        // 慢速滴注不完整 header（无结尾空行）：50ms/字节的间隔 < 读超时，
        // 此阶段连接存活（单次 read 级超时不杀仍在出字节的连接）；
        // 之后完全静默，超过读超时后服务端必须断开。
        let mut slow = TcpStream::connect(("127.0.0.1", port)).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        for &b in b"GET /api/health HT" {
            if slow.write_all(&[b]).is_err() {
                break; // 服务端提前断开也算达成，后面统一判定
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // 静默 1.2s > 400ms 读超时；此后读：tiny_http TimedOut 分支的 408，或 EOF
        std::thread::sleep(Duration::from_millis(1200));
        let mut buf = [0u8; 512];
        match slow.read(&mut buf) {
            Ok(0) => {} // EOF：连接已被服务端关闭
            Ok(n) => assert!(
                buf[..n].starts_with(b"HTTP/1.1 408"),
                "读超时后服务端应回 408 或直接断开，实际: {}",
                String::from_utf8_lossy(&buf[..n])
            ),
            Err(e) => panic!("静默超读超时后连接应被断开（408/EOF），实际读错误: {e}"),
        }

        // accept 循环未被堵死：新连接正常服务（/api/health 免鉴权）
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        c.read_to_string(&mut resp).unwrap();
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "slowloris 连接被断开后 accept 循环应仍服务新请求: {resp}"
        );

        running.shutdown.store(true, Ordering::SeqCst);
    }

    /// since=5 断线重放——4 个 id<5 + id=5 本身都不重放，只推 id>5 的 6 条；
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

    /// 并发广播回归（OCR C5-AP-04）：fetch_add 与推送同临界区后，
    /// 每个 client 队列的事件序必须 = id 单调序。修复前两者分离，
    /// 并发下 id6 可先于 id5 入队 → writer 端 `id <= last_sent` 去重
    /// 静默丢迟到的 id5。确定性断言（非 timing 概率复现）。
    #[test]
    fn broadcast_concurrent_delivery_order_matches_id_order() {
        let hub = EventHub::new();
        let (tx, rx) = std::sync::mpsc::sync_channel::<(u64, Vec<u8>)>(256);
        hub.clients
            .lock()
            .unwrap()
            .push((tx, std::sync::Weak::new()));
        let h1 = hub.clone();
        let h2 = hub.clone();
        let t1 = std::thread::spawn(move || {
            for _ in 0..50 {
                h1.broadcast(serde_json::json!({"t":1}));
            }
        });
        let t2 = std::thread::spawn(move || {
            for _ in 0..50 {
                h2.broadcast(serde_json::json!({"t":2}));
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();
        let mut last = 0u64;
        let mut count = 0u32;
        while let Ok((id, _)) = rx.try_recv() {
            assert!(id > last, "乱序投递：id={id} <= last={last}");
            last = id;
            count += 1;
        }
        assert_eq!(count, 100, "100 条广播应全部入队（容量 256）");
        assert_eq!(hub.last_id(), 100);
    }
}
