//! Endpoint 处理函数 + tauri 命令（`api_handlers`）
//!
//! 完整模块组：
//! - `api_server`   : 服务生命周期 / 端口绑定 / EventHub
//! - `api_auth`     : token 管理 / 鉴权 / 开关持久化
//! - `api_handlers` : 此目录 — endpoint handlers + tauri commands
//! - `api`          : 数据层 `TaskStore` + `MemStore` + `TauriStore`
//!
//! 端点：
//! - GET    /api/health       健康检查（唯一免鉴权，仅返回服务状态）
//! - GET    /api/tasks        任务列表（默认活跃：不含回收站/归档；`?status=todo|doing|done` 按列过滤；`?trash=1` 回收站；`?archived=1` 归档；`?all=1` 全量）
//! - GET    /api/tasks/:id    单条任务
//! - POST   /api/tasks        新建任务 `{ title, note?, status?, filePath?, fileIsDir?, due?, tags? }`
//! - PUT    /api/tasks/:id    更新任务 `{ title?, note?, status?, filePath?, fileIsDir?, due?, tags?, archived?, deleted? }`
//! - DELETE /api/tasks/:id    软删（进回收站，幂等）
//! - GET    /api/events       SSE 实时推送（`?since=<事件id>` 断线重放，事件带 id）
//!
//! 任务变更后：向 SSE 客户端广播 + 向主窗口发 tasks-updated 事件（看板自动刷新）。
//! 操作日志：数据目录 `api.log`（访问 + 变更）。token 轮换：`api_rotate_token`。
//!
//! 模块分层：
//! - `types`    公共返回类型（ApiInfo / ApiStatus）
//! - `body`     请求体读取（上限 + 总时长 + 滴注防护）
//! - `ratelimit` 速率限制 + 访问/变更日志
//! - `util`     公用工具（now_ms / 字段上限 / 状态校验 / 错误分流 / 变更后回调）
//! - `handlers` HTTP 路由 + 任务 CRUD 处理器 + 序列化锁
//! - `sse`      SSE writer 生命周期 + 单客户端接入
//! - `commands` 5 个 tauri 命令（api_start/_stop/_status/_rotate_token）+ api_stop_for_exit（透传）

pub mod body;
pub mod commands;
pub mod handlers;
pub mod ratelimit;
pub mod sse;
pub mod types;
pub mod util;
pub mod validate;

// 非 cmd 入口透传给 lib.rs / api_server.rs（避免 cmd 宏 `__cmd__` 符号冲突）：
// - handle_request: api_server.rs:206 用
// - api_stop_for_exit: lib.rs:147 用
// 命令（api_start/api_stop/api_status/api_rotate_token）由 lib.rs 用全路径引用
pub use commands::api_stop_for_exit;
pub use handlers::handle_request;

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    // 跨 6 个子模块的集成测试，super::* 只拿 mod.rs 顶层项，所以这里显式列每个子模块的入口。
    use std::io::{Read as IoRead, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::api::{MemStore, TaskStore};
    use crate::api_handlers::handlers::query_param;
    use crate::api_handlers::sse::{
        register_sse_writer, stop_sse_writers, MAX_SSE_CLIENTS, SSE_STOP_JOIN_TIMEOUT,
    };
    use crate::api_handlers::util::{change_log_line, API_MAX_FILE_PATH};
    use crate::api_server::{start_api, EventHub, RunningApi};
    use crate::db;

    fn bare_task(title: &str) -> db::Task {
        db::Task {
            id: "t1".into(),
            title: title.into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column: "todo".into(),
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: None,
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            expected_updated_at: None,
        }
    }

    #[test]
    fn query_param_decodes_percent_and_plus() {
        // %XX（含 UTF-8 多字节）与 + → 空格
        assert_eq!(
            query_param("status=%E4%BB%8A%E6%97%A5&x=a+b", "status").as_deref(),
            Some("今日")
        );
        assert_eq!(query_param("x=a+b+c", "x").as_deref(), Some("a b c"));
    }

    #[test]
    fn query_param_invalid_sequence_and_repeat_key() {
        // 非法 %XX 原样保留（lossy 不吞字符）；重复键取第一个
        assert_eq!(query_param("x=%zz%4", "x").as_deref(), Some("%zz%4"));
        assert_eq!(query_param("x=1&x=2", "x").as_deref(), Some("1"));
        assert_eq!(query_param("x=%", "x").as_deref(), Some("%"));
        assert_eq!(query_param("a=1", "missing"), None);
    }

    #[test]
    fn change_log_line_escapes_title_newline() {
        // 标题含 \n 时日志行不得出现裸换行（防伪造日志行/多行撕裂）
        let line = change_log_line("created", &bare_task("标题\n[2026-01-01] forged | x"));
        assert!(!line.contains('\n'), "日志行不得含裸换行: {line:?}");
        assert!(line.contains("标题\\n"), "换行必须转义为 \\n: {line:?}");
        assert!(!line.contains("| x"), "裸管道符必须转义: {line:?}");
    }

    #[test]
    fn change_log_line_plain_title_unchanged() {
        let line = change_log_line("updated", &bare_task("普通标题"));
        assert_eq!(line, "change op=updated id=t1 status=todo title=普通标题");
    }

    #[test]
    fn api_auth_and_crud() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48821, token.clone(), store.clone(), None, None, None).unwrap();

        // 健康检查：免鉴权
        let (st, body) = http(48821, "GET", "/api/health", None, None);
        assert_eq!(st, 200);
        assert!(body.contains("wmessage-api"));

        // 无 token / 错 token → 401
        assert_eq!(http(48821, "GET", "/api/tasks", None, None).0, 401);
        assert_eq!(http(48821, "GET", "/api/tasks", Some("wrong"), None).0, 401);

        // 空列表
        let (st, body) = http(48821, "GET", "/api/tasks", Some(&token), None);
        assert_eq!(st, 200);
        assert_eq!(body.trim(), "[]");

        let (st, body) = http(
            48821,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"测试任务","status":"doing"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        assert_eq!(v["title"], "测试任务");
        assert_eq!(v["status"], "doing");
        assert_eq!(v["column"], "doing");

        // 单条
        let (st, _) = http(
            48821,
            "GET",
            &format!("/api/tasks/{id}"),
            Some(&token),
            None,
        );
        assert_eq!(st, 200);

        // 更新：title + status done → 记完成时间
        let (st, body) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"title":"改过","status":"done"}"#),
        );
        assert_eq!(st, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "改过");
        assert_eq!(v["status"], "done");
        assert!(v["completedAt"].is_number());

        // 非法 status → 400
        let (st, _) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"status":"bad"}"#),
        );
        assert_eq!(st, 400);

        // 更新：due + tags
        let (st, body) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"due":"2026-08-20T09:00","tags":["a","b"]}"#),
        );
        assert_eq!(st, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["due"], "2026-08-20T09:00");
        assert_eq!(v["tags"][0], "a");

        // 更新：归档 / 恢复
        let (st, body) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"archived":true}"#),
        );
        assert_eq!(st, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["archived"],
            true
        );
        let (st, _) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"archived":false}"#),
        );
        assert_eq!(st, 200);

        // DELETE 软删 → 默认列表排除，?trash=1 可见，重复删幂等
        let (st, _) = http(
            48821,
            "DELETE",
            &format!("/api/tasks/{id}"),
            Some(&token),
            None,
        );
        assert_eq!(st, 200);
        let (st, body) = http(48821, "GET", "/api/tasks", Some(&token), None);
        assert_eq!(st, 200);
        assert!(!body.contains(&id), "默认列表应排除回收站任务");
        let (st, body) = http(48821, "GET", "/api/tasks?trash=1", Some(&token), None);
        assert_eq!(st, 200);
        assert!(body.contains(&id), "trash=1 应包含回收站任务");
        let (st, _) = http(
            48821,
            "DELETE",
            &format!("/api/tasks/{id}"),
            Some(&token),
            None,
        );
        assert_eq!(st, 200, "重复删除应幂等 200");

        // 恢复：deleted=false
        let (st, _) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"deleted":false}"#),
        );
        assert_eq!(st, 200);

        // 列表过滤：非法 status → 400
        let (st, _) = http(48821, "GET", "/api/tasks?status=bad", Some(&token), None);
        assert_eq!(st, 400);

        // ?status=done 只含恢复后的完成态任务（该任务之前被改到 done）
        let (st, body) = http(48821, "GET", "/api/tasks?status=done", Some(&token), None);
        assert_eq!(st, 200);
        assert!(body.contains(&id));

        // 不存在 → 404
        let (st, _) = http(48821, "GET", "/api/tasks/nope", Some(&token), None);
        assert_eq!(st, 404);

        running.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = running.handle.take() {
            let _ = h.join();
        }
    }

    #[test]
    fn sse_receives_change_events() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48822, token.clone(), store.clone(), None, None, None).unwrap();

        // 建立 SSE 连接
        let mut s = std::net::TcpStream::connect(("127.0.0.1", 48822)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\r\n"
        )
        .unwrap();
        let mut buf = [0u8; 4096];
        let mut acc = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut got = false;
        while std::time::Instant::now() < deadline {
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            acc.push_str(&String::from_utf8_lossy(&buf[..n]));
            if acc.contains("connected") {
                got = true;
                break;
            }
        }
        assert!(got, "SSE 首事件未收到，实际内容：{acc}");

        // 另一连接 POST → SSE 应收到 tasks-changed
        http(
            48822,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"sse任务"}"#),
        );
        let mut acc = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut ok = false;
        while std::time::Instant::now() < deadline {
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            acc.push_str(&String::from_utf8_lossy(&buf[..n]));
            if acc.contains("tasks-changed") {
                ok = true;
                break;
            }
        }
        assert!(ok, "SSE 未收到任务变更事件，实际内容：{acc}");

        running.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = running.handle.take() {
            let _ = h.join();
        }
    }

    // ── SSE writer 生命周期（stop 通知 + 带超时 join + 泄漏审计）──

    /// 测试专用 hub 分组键：用本地 Arc 地址保证与并行测试的真实 hub 不撞
    fn test_hub_key() -> usize {
        Arc::as_ptr(&Arc::new(())) as usize
    }

    #[test]
    fn sse_writer_stop_exits_within_timeout() {
        let key = test_hub_key();
        // 模拟一个不主动退出的 writer：事件永不来，只在 1s tick 上轮询 stop 标志
        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = stop.clone();
        let (_tx, rx) = sync_channel::<Vec<u8>>(1);
        let handle = std::thread::spawn(move || loop {
            if stop_w.load(Ordering::SeqCst) {
                break;
            }
            let _ = rx.recv_timeout(Duration::from_secs(1));
        });
        register_sse_writer(key, stop, handle);
        let mut audits: Vec<String> = Vec::new();
        let start = Instant::now();
        stop_sse_writers(key, SSE_STOP_JOIN_TIMEOUT, &mut |l: &str| {
            audits.push(l.to_string())
        });
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "writer 未在 stop 后及时退出：{:?}",
            start.elapsed()
        );
        assert!(audits.is_empty(), "正常退出不应记泄漏审计: {audits:?}");
    }

    #[test]
    fn sse_writer_stuck_detaches_with_leak_audit() {
        let key = test_hub_key();
        // 卡死 writer（不轮询 stop）：join 超时后必须 detach + ERROR 审计，不得永久挂住
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn(move || std::thread::sleep(Duration::from_secs(30)));
        register_sse_writer(key, stop, handle);
        let mut audits: Vec<String> = Vec::new();
        let start = Instant::now();
        stop_sse_writers(key, Duration::from_millis(300), &mut |l: &str| {
            audits.push(l.to_string())
        });
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "卡死 writer 不得拖住 stop：{:?}",
            start.elapsed()
        );
        assert!(
            audits.iter().any(|l| l.contains("sse_writer_leaked")),
            "缺 sse_writer_leaked 审计: {audits:?}"
        );
    }

    // ── 空 title 400 / trim 存储 / SSE 尸体收割 ──

    fn shutdown_server(running: &mut RunningApi) {
        running.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = running.handle.take() {
            let _ = h.join();
        }
    }

    /// 显式传 trim 后为空的 title 按 400 拒绝（与 create 语义一致）
    #[test]
    fn update_blank_title_returns_400() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48823, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, body) = http(
            48823,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"正常任务"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();

        // 空白 title → 400（不是静默忽略）；不传 title → 200 不动标题
        let (st, _) = http(
            48823,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"title":"   "}"#),
        );
        assert_eq!(st, 400, "空白 title 应 400");
        let (st, body) = http(
            48823,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"note":"只改备注"}"#),
        );
        assert_eq!(st, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "正常任务", "不传 title 不应动标题");

        shutdown_server(&mut running);
    }

    /// create 的 note / filePath 首尾空白不得进库（trim 后存储）
    #[test]
    fn create_trims_note_and_file_path() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48824, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, body) = http(
            48824,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"t","note":"  备注内容  ","filePath":" /tmp/x.pdf "}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["note"], "备注内容", "note 应 trim 后存储: {body}");
        assert_eq!(
            v["filePath"], "/tmp/x.pdf",
            "filePath 应 trim 后存储: {body}"
        );

        shutdown_server(&mut running);
    }

    /// clients 里塞满死连接尸体（writer 已退出 = Weak 失效）时，
    /// 新 SSE 连接应先收割尸体再判容量——否则直接 503 直到下次广播自愈
    #[test]
    fn sse_dead_clients_pruned_before_capacity_check() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        // 塞满 MAX_SSE_CLIENTS 个尸体（Weak::new() 永不 upgrade = writer 已死）
        {
            let hub = store.event_hub();
            let mut clients = hub.clients.lock().unwrap();
            for _ in 0..MAX_SSE_CLIENTS {
                let (tx, _rx) = sync_channel::<(u64, Vec<u8>)>(1);
                clients.push((tx, std::sync::Weak::new()));
            }
            assert_eq!(clients.len(), MAX_SSE_CLIENTS);
        }
        let mut running = start_api(48825, token.clone(), store.clone(), None, None, None).unwrap();

        // 新连接：尸体被收割后应正常接入（200 + connected 首事件），而非 503
        let mut s = std::net::TcpStream::connect(("127.0.0.1", 48825)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\r\n"
        )
        .unwrap();
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "尸体占满名额时新连接仍应接入（200），实际：{head}"
        );

        // 尸体被清、新连接占位：clients 应只剩 1 个活连接
        let hub = store.event_hub();
        let alive_count = hub
            .clients
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, w)| w.upgrade().is_some())
            .count();
        assert_eq!(alive_count, 1, "尸体应被收割，仅剩新连接: {alive_count}");

        drop(s);
        shutdown_server(&mut running);
    }

    /// 测试基建：MemStore 已按 db.rs 语义比对 RMW 基线；本 store 在 upsert 内
    /// 先模拟「读快照→写回」窗口里的并发写（把目标行 updated_at 推进），
    /// 使 handler 锁内 load 的基线在 upsert 时必然过期 → 走通 409 路径
    struct SabotageStore {
        inner: MemStore,
    }
    impl TaskStore for SabotageStore {
        fn load(&self) -> Result<Vec<db::Task>, String> {
            self.inner.load()
        }
        fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String> {
            for t in &tasks {
                let mut g = self.inner.tasks.lock().unwrap();
                if let Some(x) = g.iter_mut().find(|x| x.id == t.id) {
                    x.updated_at = Some(x.updated_at.unwrap_or(0) + 1);
                }
            }
            self.inner.upsert(tasks)
        }
        fn event_hub(&self) -> &Arc<EventHub> {
            self.inner.event_hub()
        }
        fn notify_change(&self, op: &str, task: &db::Task) {
            self.inner.notify_change(op, task);
        }
    }

    /// 基线外有写者插队 → PUT 回 409 且不覆盖对方修改。
    /// 覆盖两种基线：正常行（updated_at 时间戳基线）与 NULL 老行（行存在性基线）。
    #[test]
    fn update_conflict_returns_409() {
        // 预塞一条 updated_at 为 NULL 的老行（迁移前遗留）
        let inner = MemStore {
            tasks: Mutex::new(vec![bare_task("老行任务")]),
            hub: EventHub::new(),
        };
        let store: Arc<dyn TaskStore> = Arc::new(SabotageStore { inner });
        let token = "test-token-123".to_string();
        let mut running = start_api(48826, token.clone(), store.clone(), None, None, None).unwrap();

        // NULL 老行：行存在性基线 + 插队写 → 409
        let (st, body) = http(
            48826,
            "PUT",
            "/api/tasks/t1",
            Some(&token),
            Some(r#"{"title":"覆盖"}"#),
        );
        assert_eq!(st, 409, "NULL 老行被插队改必须 409: {body}");
        let tasks = store.load().unwrap();
        assert_eq!(tasks[0].title, "老行任务", "被拒写不得覆盖现行行");

        // 正常行（API 创建，updated_at 有值）：时间戳基线 + 插队写 → 409
        let (st, body) = http(
            48826,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"正常任务"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let (st, body) = http(
            48826,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"title":"覆盖"}"#),
        );
        assert_eq!(st, 409, "时间戳基线过期必须 409: {body}");
        let tasks = store.load().unwrap();
        let cur = tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(cur.title, "正常任务", "被拒写不得覆盖现行行");

        shutdown_server(&mut running);
    }

    /// NULL 老行在无并发写时可正常更新——
    /// 行存在性基线放行（不误伤正常路径）
    #[test]
    fn update_null_updated_at_row_ok() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![bare_task("老行任务")]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48827, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, body) = http(
            48827,
            "PUT",
            "/api/tasks/t1",
            Some(&token),
            Some(r#"{"title":"改好了"}"#),
        );
        assert_eq!(st, 200, "无并发写时 NULL 老行更新应放行: {body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "改好了");

        shutdown_server(&mut running);
    }

    /// `?since=abc` 解析失败必须回 400——不静默按全新连接处理
    ///（客户端会不知自己丢了重放窗口）
    #[test]
    fn sse_invalid_since_returns_400() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48828, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, _) = http(48828, "GET", "/api/events?since=abc", Some(&token), None);
        assert_eq!(st, 400, "非法 since 应 400");
        let (st, _) = http(48828, "GET", "/api/events?since=-1", Some(&token), None);
        assert_eq!(st, 400, "负数 since 应 400（u64 解析失败）");

        shutdown_server(&mut running);
    }

    /// filePath 超上限（1024 字）回 400，create / update 同规则
    #[test]
    fn file_path_over_limit_returns_400() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48829, token.clone(), store.clone(), None, None, None).unwrap();

        let long_path = "x".repeat(API_MAX_FILE_PATH + 1);
        let (st, _) = http(
            48829,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(&format!(r#"{{"title":"t","filePath":"{long_path}"}}"#)),
        );
        assert_eq!(st, 400, "create 超限 filePath 应 400");

        let (st, body) = http(
            48829,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"t"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let (st, _) = http(
            48829,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(&format!(r#"{{"filePath":"{long_path}"}}"#)),
        );
        assert_eq!(st, 400, "update 超限 filePath 应 400");

        shutdown_server(&mut running);
    }

    /// Content-Length 声明超 1MB → 立即 413，
    /// 不必等 body 读完（请求故意一字节 body 都不发：若不预拒，服务端会等 body
    /// 直到 5s 客户端读超时，测试会失败）
    #[test]
    fn oversize_content_length_rejected_early() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48830, token.clone(), store.clone(), None, None, None).unwrap();

        let mut s = std::net::TcpStream::connect(("127.0.0.1", 48830)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "POST /api/tasks HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 2000000\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(
            resp.starts_with("HTTP/1.1 413"),
            "声明超限的 body 应立即 413: {}",
            resp.lines().next().unwrap_or("")
        );

        shutdown_server(&mut running);
    }

    // ── 共享测试基建：手写 HTTP 客户端 ──

    fn http(
        port: u16,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = body.unwrap_or("");
        let mut raw =
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        if let Some(t) = token {
            raw.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        if !body.is_empty() {
            raw.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        raw.push_str("\r\n");
        raw.push_str(body);
        s.write_all(raw.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        let status = resp
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }
}
