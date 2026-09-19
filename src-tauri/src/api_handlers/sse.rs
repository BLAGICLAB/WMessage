//! SSE writer 生命周期 + 单客户端接入。
//!
//! - `SSE_WRITERS` 全局注册表：writer 线程 + stop 标志 + hub 分组键
//! - `API_HUB_KEY` 当前服务实例的 hub 身份键（api_start 时记录，api_stop 据此停 writer）
//! - `sse_connect` 单客户端：握手 → 写 connected 事件 → 断线重放 → 在线循环
//! - `stop_sse_writers` 通知指定 hub 的 writer 退出（带超时 join + 泄漏审计）

use std::io::{empty, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tiny_http::{Header, Request, Response, StatusCode};

use crate::api::TaskStore;

use super::handlers::json_err;

/// SSE 并发连接上限（每连接一个 writer 线程，不设上限可被连接洪泛耗尽线程）
pub(crate) const MAX_SSE_CLIENTS: usize = 32;

/// SSE writer 线程注册项：按 hub 分组，stop 标志 + JoinHandle。
/// writer 线程必须纳入追踪：只 join accept 线程的话，旧 hub 的 tx 不被 drop，
/// writer 循环发 keepalive——旧客户端以为活着却永远收不到新事件，
/// 且每次 rotate 累积一批泄漏线程。
pub(crate) struct SseWriterReg {
    /// hub 身份键（从 EventHub.hub_id 取，AtomicU64 计数器，跨进程单调；
    /// 不再用 Arc 指针作身份 —— Arc drop 后地址可被复用，会导致
    /// 跨 stop/start 的 writer 误关联）
    pub hub_key: u64,
    pub stop: Arc<AtomicBool>,
    pub handle: std::thread::JoinHandle<()>,
}

pub(crate) static SSE_WRITERS: Mutex<Vec<SseWriterReg>> = Mutex::new(Vec::new());

/// 当前服务实例的 hub 分组键（api_start 时记录，api_stop 据此停对应 writer）
pub(crate) static API_HUB_KEY: AtomicU64 = AtomicU64::new(0);

/// writer 退出通知的兜底 join 超时：超时仍不退出的 detach + ERROR 审计
pub(crate) const SSE_STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn register_sse_writer(
    hub_key: u64,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
) {
    // 锁 poisoning 审计:同 ratelimit::RATE 注释
    let mut g = SSE_WRITERS.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] api_handlers::sse::SSE_WRITERS: {e:?}");
        e.into_inner()
    });
    // 顺手收割已退出（客户端断开）的 writer，防注册表无界增长
    g.retain(|w| !w.handle.is_finished());
    g.push(SseWriterReg {
        hub_key,
        stop,
        handle,
    });
}

/// 停掉指定 hub 的全部 SSE writer：置 stop 标志 → 带超时 join；
/// 超时仍不退出的 drop handle（detach）并记 ERROR 审计「sse_writer_leaked」。
pub(crate) fn stop_sse_writers(hub_key: u64, timeout: Duration, audit: &mut dyn FnMut(&str)) {
    let writers = {
        // 锁 poisoning 审计:同 ratelimit::RATE 注释
    let mut g = SSE_WRITERS.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] api_handlers::sse::SSE_WRITERS: {e:?}");
        e.into_inner()
    });
        let mut taken = Vec::new();
        let mut i = 0;
        while i < g.len() {
            if g[i].hub_key == hub_key {
                taken.push(g.remove(i));
            } else {
                i += 1;
            }
        }
        taken
    };
    for w in &writers {
        w.stop.store(true, Ordering::SeqCst);
    }
    let deadline = Instant::now() + timeout;
    for w in writers {
        let h = w.handle;
        loop {
            if h.is_finished() {
                let _ = h.join();
                break;
            }
            if Instant::now() >= deadline {
                audit("sse_writer_leaked | writer join 超时未退出，已 detach");
                break; // drop(h) = detach
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// 注册 SSE 客户端：支持 `?since=<事件id>` 断线重放，然后用 tiny_http upgrade 直写。
/// 重放窗口上限 = EVENT_HISTORY（1000 条环形缓冲，api_server.rs）：溢出缺段为已知取舍。
pub(crate) fn sse_connect(req: Request, store: &Arc<dyn TaskStore>, query: &str) {
    // since 给了但 parse 失败（如 ?since=abc）回 400——
    // 不静默按全新连接处理，让客户端知道自己丢了重放窗口
    let since = match super::handlers::query_param(query, "since") {
        Some(s) => match s.parse::<u64>() {
            Ok(v) => Some(v),
            Err(_) => {
                let _ = req.respond(json_err(StatusCode(400), "since 必须是非负整数（事件 id）"));
                return;
            }
        },
        None => None,
    };
    // sync_channel(256)——单客户端最多积压 256 条，超出则丢事件（广播不阻塞）；
    // 载荷带事件 id，writer 端据此与断线重放去重
    let (tx, rx) = sync_channel::<(u64, Vec<u8>)>(256);
    // 锁中毒时用 into_inner 恢复（与 broadcast 端策略一致——静默跳过会让
    // 注册失败的 SSE 连接永远收不到事件）
    let hub = store.event_hub();
    // writer 存活令牌——注册前收割死连接尸体
    // （否则安静期内尸体占满名额 → 新连接 503）
    let alive = Arc::new(());
    {
        // 锁 poisoning 审计:同 SSE_WRITERS 注释
        let mut clients = hub.clients.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] api_handlers::sse::hub.clients: {e:?}");
            e.into_inner()
        });
        clients.retain(|(_, token)| token.upgrade().is_some());
        // SSE 连接数上限——超限 503，防连接洪泛耗尽线程
        if clients.len() >= MAX_SSE_CLIENTS {
            drop(clients);
            let _ = req.respond(json_err(StatusCode(503), "too many SSE connections"));
            return;
        }
        clients.push((tx, Arc::downgrade(&alive)));
    }
    let hub = hub.clone();
    // writer 线程纳入追踪——stop 标志供 api_stop 通知退出，JoinHandle 入注册表
    let hub_key = hub.hub_id();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_w = stop.clone();
    let handle = std::thread::spawn(move || {
        // 存活令牌随 writer 线程存活，线程退出（客户端断开/服务停止）即失效
        let _alive = alive;
        let headers = vec![
            Header::from_bytes(
                &b"Content-Type"[..],
                &b"text/event-stream; charset=utf-8"[..],
            )
            .expect("静态 header 字节不可能失败"),
            Header::from_bytes(&b"Cache-Control"[..], &b"no-cache"[..])
                .expect("静态 header 字节不可能失败"),
        ];
        let resp = Response::new(StatusCode(200), headers, empty(), Some(0), None);
        let mut stream = req.upgrade("text/event-stream", resp);
        // 写超时通过 recv_timeout 心跳 + 客户端断开检测协同处理
        // （tiny_http ResponseBox 不提供 set_write_timeout，故通过 recv 端超时兜底）。
        // recv tick 1s、每 15 tick 发一次心跳（对外节奏不变），
        // 使 stop 标志最迟 1s 内被轮询到，writer 能及时退出被 join

        // 连接成功事件（携带当前事件 id，供客户端决定下次 since 起点）
        let connected = format!(
            "data: {{\"type\":\"connected\",\"lastEventId\":{}}}\n\n",
            hub.last_id()
        );
        if stream
            .write_all(connected.as_bytes())
            .and_then(|_| stream.flush())
            .is_err()
        {
            return;
        }
        // 断线重放：补发 since 之后的历史事件
        // 重放与在线推送存在竞态——客户端注册进 clients 之后、重放快照之前
        // 广播的事件会同时出现在 history 与在线队列里。记录已发最大 id，
        // 在线循环里 id <= last_sent 的一律跳过（服务器侧去重）。
        let mut last_sent: u64 = since.unwrap_or(0);
        if since.is_some() {
            for (id, msg) in hub.replay(last_sent) {
                if stream.write_all(msg.as_bytes()).is_err() {
                    return;
                }
                if id > last_sent {
                    last_sent = id;
                }
            }
            let _ = stream.flush();
        }
        let mut idle_ticks = 0u32;
        loop {
            // 服务停止/重启时 api_stop 置位——不停则旧客户端看着 keepalive
            // 以为活着，却永远收不到新 hub 的事件
            if stop_w.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok((id, data)) => {
                    // 重放已覆盖的事件（id <= last_sent）跳过，不重复推
                    if id <= last_sent {
                        continue;
                    }
                    last_sent = id;
                    idle_ticks = 0;
                    if stream.write_all(&data).is_err() || stream.flush().is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    idle_ticks += 1;
                    if idle_ticks >= 15 {
                        idle_ticks = 0;
                        // SSE 心跳注释，保持连接存活
                        if stream.write_all(b": keepalive\n\n").is_err() {
                            break;
                        }
                        let _ = stream.flush();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    register_sse_writer(hub_key, stop, handle);
}
