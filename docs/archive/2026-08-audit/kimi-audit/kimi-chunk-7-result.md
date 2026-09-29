kimi version 0.36.1
• Let me start reading files in order. First lib.rs.

• 我先按顺序读 `src-tauri/src/lib.rs`。

• lib.rs read. Notes:
  - No cleanup on exit for API server / Python subprocesses / SSE. RunEvent::ExitRequested only destroys main window. `app.exit(0)` from tray — does that trigger cleanup? No explicit shutdown of ApiState, bot_scheduler task, migration polling, Python child processes.
  - Global shortcut: no unregister on exit (OS handles it, minor). No double-register protection beyond setup runs once.
  - bring_main_to_front has std::thread::sleep(80ms) blocking — called from shortcut handler and tray event (sync context, blocks UI thread briefly — minor, likely already known? Not in scope list; it's a UI freeze of 80ms, P2 maybe. But scope is resource lifecycle; skip or note briefly).

  Next: api_server.rs — only connection cleanup hooks. Check its size first? Just read.

• 继续读 `api_server.rs`（只看连接清理钩子）。

• api_server notes:
  - start_api: recv thread has shutdown flag checked only every 400ms (recv_timeout). RunningApi.handle: Option<JoinHandle>. Who joins it? Check api_handlers api_stop. The shutdown flag break happens only before recv_timeout — after shutdown set, loop exits within 400ms, fine. But on app exit, api_stop is never called (lib.rs ExitRequested doesn't stop API). Also, SSE client cleanup: EventHub broadcast removes dead clients on try_send Err — but only when broadcasts happen; a client whose channel never fills and who disconnects stays in `clients` vec forever (leak until next failed send). Also worker threads that timed out continue running — holding req, no limit issue due to ActiveGuard.

  Next: api_handlers.rs — check api_stop join, SSE handler loop exit, handler panic resource recovery.

• 继续读 `api_handlers.rs`。

• api_handlers notes:
  - sse_connect: on write error / disconnect, the writer thread breaks and returns — but the `tx` SyncSender stays in `hub.clients` until the next broadcast's try_send fails. That's the known P1-A2 area ("踢客户端时是否清理了它对应的订阅/锁?" — chunk 1 P1-A2 covered踢客户端的清理路径？ The prompt says chunk 1 P1-A2 known: mpsc channel 满时踢客户端的清理路径. The question asked: 踢客户端时是否清理了它对应的订阅/锁? Broadcast removes the sender — that's the only subscription. So cleaned on next broadcast. But disconnected client with empty queue: tx stays in clients vec forever (until a broadcast fails to send, which it won't because queue never fills... actually try_send on a disconnected receiver returns Err(Disconnected) — since rx is dropped when the writer thread exits, next broadcast try_send returns Err and removes it. So OK, lazily cleaned. Fine.

  - api_stop joins the recv thread, but in-flight worker threads and SSE writer threads are not tracked/joined — daemon threads die with process. api_stop sets shutdown and joins only the accept loop; SSE writer threads keep running even after api_stop! Wait — after api_stop, the server is dropped (RunningApi dropped → Server dropped?). Server is moved into the thread; thread exits loop after shutdown; Server dropped → but upgraded SSE connections are separate TcpStreams, writer threads keep looping on rx.recv_timeout; hub tx still in clients; but the store/hub are dropped when RunningApi dropped? The hub Arc is held by the writer thread (hub cloned at line 690). So after api_stop, SSE writer threads survive and keep writing keepalives to still-connected clients — the SSE connection stays alive even though the API is "stopped". Restarting api_start creates a NEW EventHub (new clients list), so old clients never get removed, old SSE threads leak until clients disconnect, and old clients stop receiving events silently. That's a real finding: api_stop / api_rotate_token (which calls api_stop then api_start) leaks SSE writer threads & leaves old SSE clients hanging without events. P1.

  - api_rotate_token: `let was_running = state.0.lock()...?.is_some()` — lock dropped at end of statement, fine. No held-lock await (no async here anyway).

  Also note api_start autostart in setup: fine.

  Next: bot_scheduler.rs.

• 继续读 `bot_scheduler.rs`。

• bot_scheduler: spawn handles are awaited sequentially — fine, no leak. The scheduler runs forever with no shutdown on exit — app exit kills the runtime; fine. No new finding beyond "no shutdown hook" (global).

  Next: bot_py.rs — check spawn failure / child process leaks.

• 继续读 `bot_py.rs`（只看子进程生命周期）。

• bot_py notes:
  - run_python: timeout path kill_tree then remove_dir_all and return Err — but the two reader threads: child killed → pipes close → threads send and exit; rx is dropped. OK. But on timeout, rx dropped — threads' send fails silently, fine.
  - On timeout path: threads might still be blocked reading if the child spawned grandchildren holding the pipe? kill_tree kills process group on unix, /T on windows, so pipes close. OK.
  - Normal path: `child.try_wait` breaks loop, then `rx.iter()` blocks until both threads finish reading. If process exited but a grandchild inherited stdout pipe and keeps it open, read_to_end blocks forever → run_python hangs forever (no timeout after exit!). Actually child exited → try_wait returns status → break. Then rx.iter() waits for reader threads; reader threads block on read until pipe closes; if the script spawned a background child inheriting stdout (e.g. `os.system("sleep 1000 &")` with stdout inherited), pipe stays open → run_python hangs indefinitely, blocking a tool call in bot_chat's async context (it's a sync fn called in async context? py_exec_sync called directly — "工具链在 async 上下文直接调用" — blocking the tokio worker thread for 60s+... actually run_python blocks with thread::sleep loops — called from async context without spawn_blocking! That blocks an async runtime worker thread for the entire run duration (up to 120s for doc_extract etc.). Is that within scope? "未 await 的 tokio 任务 / 后台任务" — blocking-in-async is relevant: sync 60-120s blocking call in async context. But is tokio runtime multi-threaded? tauri async_runtime is multi-threaded tokio, so it blocks one worker, not fatal. But the "hangs forever after child exit" case is a genuine bug: no overall deadline covering the read phase. P1 finding: run_python 超时只覆盖 try_wait 循环，子进程退出后 rx.iter() 等待读线程，若孙进程继承管道则永久挂死（无二次超时）。
  - spawn failure: `cmd.spawn()` err → dir not cleaned (run.py/params.json leftover in py-runs/<uuid>). P2-9 known issue is temp dirs not swept at startup; this adds: spawn-fail path leaks the dir immediately. Related but distinct-ish; chunk 3 P2-9 was "临时目录未清扫" generally. The spawn-fail leak is part of same root. I could mention as sub-note. Skip listing as new? The instruction says don't repeat P2-9. I'll mention briefly that spawn-fail path also leaks dir (same root as P2-9) — but they said don't list. I'll fold it into existing note or skip. Maybe one line in risk list marked as "P2-9 同根，补充新泄漏点". Borderline. I'll mention it in评估 or差异.

  Next: migration.rs.

• 继续读 `migration.rs`（只看启动期任务泄漏）。

• migration.rs: spawn_polling is a detached std::thread, no shutdown — acceptable at exit (daemon thread dies with process). journal_replay_pending opens db connections via db::open_db each call — Connection dropped per call, fine. block_on bridge from std::thread — fine.

  Now I need to check #[tauri::command] functions for resource lifecycle: tokio::spawn / channels / oneshot confirm paths (bot_slash::bot_confirm_response), HTTP client / SSE in bot_chat / bot_model_loop / bot.rs. The prompt says "所有 #[tauri::command] 函数 ... 只看资源生命周期相关". Key ones: bot_chat.rs (bot_stop, streaming HTTP client), bot_slash.rs (oneshot confirm). Also db.rs wal_checkpoint file handles.

  Do the allowed directed greps now, sequentially.src-tauri/src/audit.rs:65:pub static BOT_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
src-tauri/src/bot.rs:961:    let picked = tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/bot_chat.rs:589:    EXEC_RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
src-tauri/src/bot_skills.rs:25:/// 文件多选对话框（Rust 侧 spawn_blocking 弹框）：挂件窗口 ➕ 添加附件用。
src-tauri/src/bot_skills.rs:30:    let picked = tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/bot_skills.rs:439:    SKILL_RUNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
src-tauri/src/bot_slash.rs:25:    STOP_REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
src-tauri/src/bot_slash.rs:76:    std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>;
src-tauri/src/bot_slash.rs:80:    CONFIRMS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
src-tauri/src/bot_slash.rs:100:    let (tx, rx) = tokio::sync::oneshot::channel();
src-tauri/src/db.rs:54:            if std::fs::File::create(&probe).is_ok() {
src-tauri/src/db.rs:533:    // B3: 高频写 + 跨表事务 → 主线程会阻塞；扔到 spawn_blocking。
src-tauri/src/db.rs:534:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:571:    // B3: 长会话（几千条消息）查询会被主线程阻塞；扔到 spawn_blocking。
src-tauri/src/db.rs:572:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:640:    // B3: 长会话全量覆盖写入 + fsync 重；主线程阻塞；扔到 spawn_blocking。
src-tauri/src/db.rs:641:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:659:    // B3: DELETE 大量消息时仍可能阻塞；扔到 spawn_blocking。
src-tauri/src/db.rs:660:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:858:static DB_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
src-tauri/src/db.rs:862:    // B3: 启动加载全部任务（可能有几千条 + migrate_data_json 读 JSON 文件）；扔到 spawn_blocking。
src-tauri/src/db.rs:863:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:879:    // B3: 高频写（挂件拖拽/编辑都走这里），批量事务含 fsync；扔到 spawn_blocking。
src-tauri/src/db.rs:880:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:899:    // B3: 批量删（回收站多选 / 清空）；扔到 spawn_blocking。
src-tauri/src/db.rs:900:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:1014:    // B3: 长文件读 + 跨表事务；扔到 spawn_blocking。
src-tauri/src/db.rs:1015:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:1059:    // B3: 大数据集导出（load_all + JSON 序列化 + 文件写）阻塞主线程；扔到 spawn_blocking。
src-tauri/src/db.rs:1060:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/db.rs:1074:    // B3: 大文件读 + 解析 + 长事务；扔到 spawn_blocking。
src-tauri/src/db.rs:1075:    tauri::async_runtime::spawn_blocking(move || {
src-tauri/src/profile.rs:59:            if std::fs::File::create(&probe).is_ok() {
src-tauri/src/profile.rs:296:    static ENV_LOCK: Mutex<()> = Mutex::new(());
src-tauri/src/bot_scheduler.rs:314:    tauri::async_runtime::spawn(async move {
src-tauri/src/bot_scheduler.rs:323:                let handle = tauri::async_runtime::spawn(run_scheduled(app.clone(), t));
Only 2 async spawns, both in bot_scheduler and awaited. Good.

  Now check bot_slash.rs oneshot confirm lifecycle (lines ~70-160), and bot_chat.rs around stop registry / streaming. Read bot_slash.rs relevant part.

• 看 `bot_slash.rs` 的 oneshot 确认通道生命周期。

• bot_slash confirm path: oneshot with 60s timeout, on timeout removes from map. OK — sender dropped, receiver doesn't hang. But note: on timeout path `confirms().remove(&id)` — good. On success path, tx was removed by bot_confirm_response. Fine. No leak. 60s wait — receiver awaited with timeout. Good.

  Edge: `bot_confirm_response` can be called after timeout → remove returns None, fine.

  Now check bot_chat.rs streaming HTTP client + EXEC_RUNNING + run_python call in async context, and bot.rs HTTP client (reqwest?) — check for client reuse/timeout. Also check whether py_exec_sync is called from async context directly (blocking). Let me grep bot.rs/bot_chat.rs/bot_model_loop.rs for reqwest / Client / py_exec_sync call sites.src-tauri/src/bot.rs:1326:    match crate::bot_py::py_exec_sync(app, code.to_string(), None) {
src-tauri/src/bot_chat.rs:509:    let client = reqwest::Client::builder()
src-tauri/src/bot_chat.rs:537:        "stream": false
src-tauri/src/bot_model_loop.rs:289:    let client = reqwest::Client::builder()
src-tauri/src/bot_model_loop.rs:317:    // 上轮 streamed 文本快照（Block 2 接入，2026-08-17 22:26）：
src-tauri/src/bot_model_loop.rs:319:    let mut last_streamed = String::new();
src-tauri/src/bot_model_loop.rs:333:                    return Ok((last_streamed.clone(), collected_refs));
src-tauri/src/bot_model_loop.rs:337:                    return Ok((last_streamed.clone(), collected_refs));
src-tauri/src/bot_model_loop.rs:341:                    return Ok((last_streamed.clone(), collected_refs));
src-tauri/src/bot_model_loop.rs:345:                    return Ok((last_streamed.clone(), collected_refs));
src-tauri/src/bot_model_loop.rs:353:            "stream": true
src-tauri/src/bot_model_loop.rs:394:        let mut stream = resp.bytes_stream();
src-tauri/src/bot_model_loop.rs:403:        while let Some(chunk) = stream.next().await {
src-tauri/src/bot_model_loop.rs:584:        // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
src-tauri/src/bot_model_loop.rs:585:        last_streamed = final_text.clone();
Continue: check bot_model_loop.rs reqwest client config (timeouts, connection reuse per call — a new Client per model call leaks nothing but no pooling; check timeout). Also check py_exec_sync call site context in bot.rs:1326 (called from async tool loop → blocking). And check EXEC_RUNNING guard in bot_chat.rs. Let me read the relevant slices.reqwest client per run_model_loop call — new client per chat (no pooling, minor). timeout 300s covers whole request including streaming? reqwest `.timeout()` is total request timeout including body streaming — so a long stream is capped at 300s. OK, no hang.

  Key remaining checks:
  1. py_exec_sync called at bot.rs:1326 from async context — blocking sync call up to 60s in async context (blocks tokio worker). Also run_python's post-exit read hang (grandchild holds pipe) → indefinite block of a tokio worker thread.
  2. EXEC_RUNNING in bot_chat.rs — check guard.
  3. oneshot confirm — fine.
  4. Exit hooks: lib.rs RunEvent::ExitRequested only destroys main window; API server not stopped (api_stop not called) — but process exit kills threads anyway; the real issue is api_stop/rotate leaking SSE writer threads (alive SSE connections keep sending keepalives, clients think API is up but get no events; also old hub never cleaned). Also on rotate_token restart: new hub, old SSE threads still loop on old hub's rx — thread leak until client disconnects.
  5. Global shortcut: no unregister on exit (OS cleans up on process exit — fine, minor). Registered once in setup — fine.

  Check bot_chat.rs EXEC_RUNNING guard and where run_python/py_exec_sync invoked. Read bot.rs around 1326 context, and bot_chat.rs 500-620.tool_run_python is a sync fn called from the async tool loop — need to confirm it's called directly (not via spawn_blocking) in bot.rs tool dispatch. It's a `fn` (sync) called from async run_model_loop context presumably. run_python blocks up to 60s (default) with thread::sleep polling — on a tokio worker thread this blocks that worker for the duration. And the post-exit read hang: if grandchild inherits stdout pipe, rx.iter() blocks forever → tokio worker permanently blocked, and StopGuard can't interrupt (sync code doesn't check stop flag). P1: run_python 无整体兜底超时（退出后读阶段无超时）+ 在 async 上下文同步阻塞。

  Let me quickly verify how tool_run_python is dispatched (async context, not spawn_blocking). Grep tool dispatch.327:        "run_python" => tool_run_python(app, args),
961:    let picked = tauri::async_runtime::spawn_blocking(move || {
1320:fn tool_run_python(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
Confirmed: `tool_run_python` is sync, called directly in the async `execute_tool` — blocks the tokio worker thread for up to 60s (and potentially forever in the post-exit read hang case). Note also tool_create_word etc. call run_python (sync 120s) — they're async fns but internally call sync run_python directly (doc_make_word is a tauri async command that calls run_python directly without spawn_blocking!). E.g. doc_make_word at bot_py.rs:872 — `pub async fn doc_make_word` calls `run_python(...)` directly — blocks worker up to 120s. Same pattern.

  Also check tool_extract_document etc. in bot.rs — likely call run_python similarly.

  Now check db.rs wal_checkpoint / connection close — grep for open_db and checkpoint.80:pub fn open_db(app: &tauri::AppHandle) -> Result<rusqlite::Connection, String> {
90:                if let Ok(conn) = rusqlite::Connection::open(&legacy_db) {
91:                    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
97:    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
1019:        let src = rusqlite::Connection::open_with_flags(
1144:        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
1311:        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
1370:        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
1443:        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
db: open per call, Connection dropped → closed. No long-lived handles. Fine.

  I have enough. Check global shortcut unregister — not done, but process exit releases it; double registration protected by setup running once. Minor/no issue.

  One more: lib.rs bring_main_to_front std::thread::sleep(80ms) — called from global shortcut handler (runs on main thread?) — the shortcut handler runs on the main thread in tauri plugin; 80ms sleep blocks the event loop briefly. Minor P2, within lifecycle scope? It's a UI freeze, not resource. I could mention as P2 but it may already be known. Not in the skip list explicitly. It's marginal — 80ms freeze on every hotkey press. I'll mention as P2 briefly.

  Also: RunEvent::ExitRequested destroys main window but doesn't stop API server / kill Python children. On real exit, running Python child processes (spawned with piped stdout, process group) are NOT killed — app exit orphans running python processes (up to 60-120s, or forever if grandchild holds pipes). Since kill on exit isn't wired, a python script running at quit time becomes orphaned. P2/P1. Given run_python can hang forever (read-phase), orphan could be permanent. Tie together.

  Also tokio runtime: on app exit, in-flight bot_chat streams — reqwest client dropped with runtime shutdown; fine.

  SSE writer thread leak on api_stop/rotate: confirmed real. Writer thread holds hub clone; after api_stop, threads still loop recv_timeout 15s writing keepalive; rx never disconnected (tx remains in old hub clients vec, hub kept alive by thread's Arc). Actually wait — the writer holds `hub` Arc and rx. The tx lives in hub.clients. Nothing drops tx → recv_timeout never returns Disconnected → thread loops forever writing keepalives until the TCP write fails (client disconnect). So after api_stop with a connected SSE client that never disconnects, the writer thread lives forever, and the client keeps receiving keepalives — believing API alive — but never gets events (new hub on restart). That's a genuine P1.

  Also api_start autostart: fine.

  Statistics:
  - tokio::spawn / async_runtime::spawn total: bot_scheduler 2 (both awaited). spawn_blocking many (db ~10, migration 3, bot 1, bot_skills 1, bot_py 2) — all awaited via .await. Unheld handles: scheduler main loop spawn (handle dropped — intentional daemon). run_scheduled spawn handle awaited. So 未持句柄: 1 (scheduler daemon, by design).
  - std::thread::spawn: api_server recv thread (handle held, joined in api_stop), per-request worker threads (detached, guarded by ActiveGuard), SSE writer threads (detached, NOT tracked → leak on api_stop), bot_py reader threads (detached but bounded by channel), migration polling thread (daemon, no shutdown — by design).
  - Mutex total (in scope files): ApiState, EventHub clients+history, RATE, SCHED_RUNNING, EXEC_RUNNING, STOP_REGISTRY, CONFIRMS, SKILL_RUNS, DB_WRITE_LOCK, BOT_LOG_LOCK, profile ENV_LOCK ≈ 12. 持锁 await: 0 (all locks are std::sync::Mutex held across sync code only; confirmed none held across .await — api_start locks dropped before await-free code; it's sync fn anyway). One nuance: api_rotate_token locks state.0 in a temporary — fine.
  - 文件 open 未 close: 0 (all scoped).

  New findings:
  1. [P1] api_handlers.rs:691 + api_stop (791): api_stop/api_rotate_token 只 join accept 线程，SSE writer 线程不被追踪/不通知；旧 hub 的 tx 永不 drop → writer 线程永久循环发 keepalive，旧客户端以为连接活着却永远收不到新事件；rotate_token 每次重启泄漏 N 个线程。建议：api_stop 时清空 hub.clients（drop 所有 tx → writer 收到 Disconnected 退出），或 RunningApi 持有 hub 引用。
  2. [P1] bot_py.rs:259-282 + bot.rs:327: run_python 超时只覆盖 try_wait 阶段；子进程退出后 rx.iter() 等读线程，若孙进程继承 stdout/stderr 管道则永久阻塞；且整个同步调用直接在 async 工具循环/命令上下文执行（doc_* 命令也一样），冻结 tokio worker（StopGuard 无法中断）。建议：整体 deadline 包住读阶段 + 走 spawn_blocking。
  3. [P2] lib.rs:393-401: RunEvent::ExitRequested 只销毁主窗口，不停止 API 服务、不杀在跑的 Python 子进程（子进程变孤儿，配合上条可永久残留）。建议：退出钩子调 api_stop + kill 活跃子进程。
  4. [P2] bot_py.rs:233: spawn 失败路径临时目录（含 run.py/params.json）不清理（与 P2-9 同根但属新泄漏点——P2-9 是"启动时无清扫"，这里是"失败路径即时泄漏"，即使加了启动清扫，运行期仍会积累）。Hmm, instruction says don't repeat P2-9. This is arguably distinct. I'll mention it briefly marked as P2-9 补充.
  5. [P2] lib.rs:57: bring_main_to_front 在主线程 sleep 80ms（快捷键/托盘回调路径），每次按热键冻结 UI 事件循环 80ms。建议：always_on_top 恢复改异步延迟。

  Is #5 already covered in previous audits? Unknown; it's minor, mention as P2.

  Also check: oneshot 确认 — verified safe (60s timeout + remove). Mention "无问题" in evaluation.

  Duplicate count 0. New findings: 4 (or 5 counting the spawn-fail note).

  Keep output ≤50 lines.

• # Chunk 7 审计结果：资源生命周期 + 后台任务

  ## 风险清单

  - [P1] `api_handlers.rs:791` + `api_handlers.rs:691` — `api_stop`/`api_rotate_token` 只 join accept 线程，**SSE writer 线程不被追踪也不被通知**：旧 hub 的 `tx` 留在 `clients` 里（writer 线程自身持 `Arc<EventHub>` 保活），`rx` 永远收不到 `Disconnected`，线程每 15s 发 keepalive 空转直到客户端断开；期间旧客户端以为连接活着却**永远收不到新事件**（重启后是新 hub）。每次 rotate token 泄漏 N 个线程。**建议**：`api_stop` 时清空 `hub.clients`（drop 所有 tx，writer 立即退出），或 `RunningApi` 持有 hub 句柄统一关停。
  - [P1] `bot_py.rs:259-282` + `bot.rs:327` — `run_python` 超时只覆盖 `try_wait` 轮询阶段；子进程退出后 `rx.iter()` 等读线程，**若脚本留了继承 stdout/stderr 管道的孙进程，读线程永久阻塞 → 整个调用永久挂死**（无二次超时）。且该同步函数在 async 工具循环/`doc_*` 命令上下文直接调用（`bot_py.rs:849/881/938/...` 同样），阻塞 tokio worker 最长 120s，`StopGuard` 无法中断。**建议**：整体 deadline 包住读阶段 + 调用走 `spawn_blocking`。
  - [P2] `lib.rs:393-401` — `RunEvent::ExitRequested` 只销毁主窗口，**无退出清理**：不 `api_stop`（API flag 文件残留→下次自动恢复尚可，但运行中 SSE/线程靠进程强杀）、不杀在跑的 Python 子进程（变孤儿，配合上条可永久残留）。**建议**：退出钩子里停 API + kill 活跃子进程。
  - [P2] `bot_py.rs:233` — `cmd.spawn()` 失败路径直接 `?` 返回，临时目录（含 `run.py`/`params.json`）**即时泄漏**（P2-9 讲的是无启动清扫机制；此处是运行期失败路径新增泄漏，即使补了启动清扫仍会积累）。
  - [P2] `lib.rs:57` — `bring_main_to_front` 在主线程 `sleep(80ms)`（快捷键/托盘回调路径），每次唤起冻结 UI 事件循环 80ms。**建议**：`always_on_top` 恢复改异步延迟。

  ## 整体评估
  后台任务骨架（scheduler / migration / 确认通道 / 各类 RAII 守卫）做得相当扎实，真正的架构级根因集中在两处：**SSE writer 线程与 API 服务生命周期脱钩**，以及 **Python 子进程缺少覆盖读阶段的整体兜底超时**——这两个都能解释"老板遇到的莫名挂死/状态不一致"类 bug。

  ## 与已有审计差异
  重复 0 条，新发现 5 条（其中 spawn 失败泄目录为 P2-9 的补充泄漏点，非同一条）。

  ## 统计
  - `async_runtime::spawn` 共 2 处（bot_scheduler），句柄 1 个故意丢弃（daemon 主循环）、1 个已 await；`spawn_blocking` ~17 处全部 `.await`，无未持句柄
  - `std::thread::spawn`：api accept（已 join）/ worker（ActiveGuard 兜底）/ **SSE writer（未追踪，泄漏）** / py 读线程（有界）/ migration 轮询（daemon）
  - `oneshot` 确认通道：60s `tokio::time::timeout` + 超时 remove，sender/receiver 均不挂死 —— 无问题
  - `Mutex` 共 ~12 个（ApiState、EventHub×2、RATE、SCHED_RUNNING、EXEC_RUNNING、STOP_REGISTRY、CONFIRMS、SKILL_RUNS、DB_WRITE_LOCK、BOT_LOG_LOCK、ENV_LOCK），**持锁 await 0 处**
  - 文件 open 未 close 0 处（rusqlite Connection / fs 句柄均作用域内 drop）

To resume this session: kimi -r session_7d5c5bb3-d50d-4697-be25-15f4cda0ec3a
