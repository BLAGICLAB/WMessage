kimi version 0.36.1
• Let me read the three files.

• 我先读取三个目标文件。

• Read the remaining lines (tests, mostly, but check if anything after line 1000 matters).Now let me analyze for architectural/security/boundary issues, avoiding repeats of existing audits. Let me quickly check existing audit docs to see what's already covered so I don't duplicate. The user said don't repeat P1/P2 items. Let me skim the fix plan to know what's already found. Actually the instructions say skip list is specific; but to avoid duplicates I should check the audit docs. Let me grep quickly.

• 我快速核对已有审计报告，避免重复。docs/AUDIT-REPORT-MANUAL-2026-08-18.md:39:  api.rs           (1281) 本地 HTTP API + SSE
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:59:### ⚠️ P0-2：`api.rs` 1281 行做 HTTP API + SSE 单文件
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:62:- **问题**：tiny_http 服务 + SSE 事件流 + 鉴权 + 限流 + 任务 CRUD 全堆一起
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:64:- **建议**：拆 `api_server.rs` / `api_sse.rs` / `api_auth.rs` 3 文件。**非 release blocker**
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:91:- **验证**：cargo check 0 错 + cargo test --lib 158 passed（无回归）
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:225:| pytest 静态分支断言 | 24 passed + 1 skipped | tests-audit/audit_pre_step_pre_execute.py |
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:258:| 2 | P0 | 架构 | src-tauri/src/api.rs:1-1281 | HTTP API + SSE 单文件，建议拆 3 文件 | ✅ `a58562d`（拆 api_server / api_auth / api_handlers） |
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:259:| 3 | P0 | 逻辑 | src-tauri/src/*.rs 130 处 unwrap/expect | 重点审 api.rs / bot.rs 的 expect，Tauri command panic 会导致前端白屏 | ✅ `e24515d`（全仓130 →100，部分已换 `?` + `unwrap_or`，剩余 100 多在 bot_skills 37 + migration 28 + api_handlers 17 里 / 多数是 file IO 不崩进程） |
docs/AUDIT-REPORT-MANUAL-2026-08-18.md:302:**测试状态**：cargo test --lib 172 passed（8 error.rs 新测 + 原 164），tsc 干净。
docs/AUDIT-AGENT-CORE-2026-08-18.md:84:            ├─ SSE 流解析（parse_sse_chunk）：
docs/AUDIT-AGENT-CORE-2026-08-18.md:120:- pytest 静态分支断言覆盖（tests-audit/audit_pre_step_pre_execute.py 24 passed + 1 skipped）
docs/AUDIT-AGENT-CORE-2026-08-18.md:224:│       ├─ SSE 流解析（parse_sse_chunk）：                           │
docs/AUDIT-AGENT-FLOW-2026-08-18.md:5:> **审计范围**：bot_chat / bot_model_loop / bot_scheduler / bot_slash / api_server / api_auth / api_handlers + errorHandler + CI钩子
docs/AUDIT-AGENT-FLOW-2026-08-18.md:49:  │    ├─ SSE 解析（parse_sse_chunk + feed_think 拆 <think>）：
docs/AUDIT-AGENT-FLOW-2026-08-18.md:142:[api_handlers.rs::api_start]
docs/AUDIT-AGENT-FLOW-2026-08-18.md:143:  └─ start_api(API_PORT, token, store, emit, log_path, on_error)
docs/AUDIT-AGENT-FLOW-2026-08-18.md:144:       ├─ token (API_PORT=4763, 强绑 127.0.0.1)
docs/AUDIT-AGENT-FLOW-2026-08-18.md:148:       │    │    ├─ verify_bearer(req, token) → 401 if fail
docs/AUDIT-AGENT-FLOW-2026-08-18.md:149:       │    │    ├─ 路径分发：/api/health /api/tasks /api/events /api/tasks/:id
docs/AUDIT-AGENT-FLOW-2026-08-18.md:155:[外部机器人 curl http://127.0.0.1:4763/api/tasks -H "Authorization: Bearer <token>"]
docs/AUDIT-AGENT-FLOW-2026-08-18.md:160:**结论**：✅ 鉴权 + 限流 + SSE + 审计事件齐全（f63e81a 修复 eprintln 后）。
docs/AUDIT-AGENT-FLOW-2026-08-18.md:245:### ⚠️ 优化 3：bot_model_loop 与 bot_chat **跨模块信号传递只靠 SSE emit**
docs/AUDIT-AGENT-FLOW-2026-08-18.md:307:| **功能正确** | HTTP API 鉴权 + 限流 + SSE + 审计齐全 | ✅ |


• The flow audit mentions 鉴权+限流+SSE 齐全 (high-level). Now let me analyze deeply for new findings.

  Findings analysis:

  1. **Rate limit placement bug** (api_handlers.rs:79-94): `/api/health` bypasses rate limit (checked before rate_check). Actually health is before rate limit — so health is both unauthenticated AND unrate-limited. That's noted in the task focus. Is it abusable? It returns static JSON; an uncontrolled loop could hammer it but tiny_http single-threaded recv loop means it can block other requests — DoS vector. Also more importantly: **rate limiter is checked BEFORE auth** — meaning an unauthenticated attacker on loopback can exhaust the 120/min budget, denying service to the legitimate token holder. That's a real design flaw: 401 responses consume the rate budget. Rate limit should be per-token or after auth. Actually also: rate limit is global across all endpoints, not per-connection — a malicious local process can starve the legit bot. P1.

  2. **SSE connection consumes rate limit once but no limit on SSE connections**: `sse_connect` spawns a thread per connection and pushes a Sender into hub.clients. No cap on concurrent SSE clients. Each connection = one thread + one channel. An authenticated client (or a token-leaked one) can open thousands of SSE connections → thread exhaustion / memory. Also dead-client cleanup: clients removed only on `send` failure in broadcast (retain). mpsc channel is unbounded — if client is connected but slow/not reading, `tx.send` succeeds and queues messages in memory unboundedly → memory leak per slow client. Actually send on unbounded channel never fails unless receiver dropped. Receiver (rx) is held by the SSE writer thread; if the TCP write blocks (client not reading), writer thread blocks in write_all forever, rx never dropped, tx.send keeps queuing → unbounded memory growth in channel. That's a real P1 memory-leak root cause. tiny_http upgrade stream — write to a socket with full buffer blocks. No write timeout. So a stalled SSE client → writer thread blocks → channel grows unboundedly → memory leak. P1.

  3. **SSE client registered before upgrade/auth... auth is done before. But registration happens before the first write succeeds** — if the client disconnects immediately, sender stays in clients until next broadcast fails. Minor.

  4. **Broadcast holds clients lock while sending** — send on unbounded channel is non-blocking, fine. But history + message clone per client — fine.

  5. **update_task partial-validation ordering**: validations happen after store.load; fine.

  6. **update_task title empty string silently ignored** — `if !tt.is_empty()` — a PUT with `title: ""` is silently a no-op rather than 400; inconsistent with create (400). Minor boundary inconsistency, P2 maybe. Actually that's arguably intentional (can't unset title). Not architecture.

  7. **Race: store.load() then upsert** — read-modify-write across whole task list without locking. Concurrent API requests (single-threaded recv loop — actually server.recv is single-threaded, and requests handled sequentially except SSE. So requests are serialized by the single recv loop. Wait — handle_request is called in the recv loop thread, and handlers run synchronously; only SSE spawns a thread. So writes are serialized. OK, no race within API. But races with bot.rs writes through TauriStore (app state) — different chunk's scope.

  8. **Token comparison not constant-time** (api_auth.rs:72): `h.value.as_str() == expected` — timing attack. Loopback-only, low severity, P2. Also multiple Authorization headers: `.any()` — if any matches, OK; that's fine-ish.

  9. **Token stored plaintext in api-token.txt** with default file permissions (no 0600). On macOS data dir perms; file written with default umask — world-readable potentially (0644). P2. Also `ApiStatus`/`ApiInfo` return token to frontend via tauri command — fine (local UI).

  10. **401 vs error leakage**: 401 for all auth failures, uniform "unauthorized" — good. No 403. Fine.

  11. **health endpoint before rate limit**: as said, unauthenticated + unrate-limited, can be hammered to keep the single-threaded loop busy → DoS of the whole API since all request handling is single-threaded. Actually recv loop: each request handled synchronously in loop; health is cheap. But combined with #1 (rate limit before auth), the bigger issue is budget exhaustion. Note `/api/health` skipping rate limit is arguably fine, but with single-threaded server any endpoint flood blocks everything.

  12. **Single-threaded request handling + blocking SSE writes**: handle_request runs in the one recv thread. sse_connect spawns a thread for the stream, good. But `read_body_limited` reads body synchronously with no timeout — a client that opens a connection and sends headers slowly / never completes body blocks the entire API server (slowloris). tiny_http recv_timeout only covers accept. `read_to_end` on request body blocks indefinitely → single-threaded server fully stalled by one stalled connection. That's a P1/P0 architectural DoS — no read timeout on the socket. Does tiny_http set read timeout? By default tiny_http requests block. Yes, a slow POST stalls all API. This is a genuine root-cause bug class: single-threaded loop + unbounded blocking reads. P1.

  13. **SSE replay between registration and connected event**: client registers, then replay from history — events broadcast between `hub.last_id()` read and replay could be duplicated (sent both via replay and live channel). Duplicate events, minor. Also events broadcast after replay() snapshot but before entering loop are queued in channel — fine. Duplicates are tolerable (id-based dedupe possible client-side). P2.

  14. **EventHub per start_api instance; broadcast only from API handlers** — task changes made via UI/bot while API running don't hit hub (hub created in start_api, only accessible via handlers). So SSE clients miss changes made from the app UI! Is that true? hub is local to start_api closure; broadcast called only in after_change (API writes). UI edits → TauriStore directly → no SSE event. So external bot listening on /api/events never learns about UI-side changes. That's an architectural gap — SSE "实时推送" only covers API-originated changes. Is that covered in AGENT-FLOW? Flow 5 covered API flow high-level correctness. This could be a new finding: event source not unified — hub should be app-level. P1 as architecture (root cause of "bot 不知道用户改了任务" class bugs). Worth listing.

  15. **api_stop doesn't drain SSE threads**: shutdown flag stops recv loop, but SSE writer threads live on rx.recv_timeout forever since hub still alive (hub owned by closure thread — when loop breaks, hub dropped, clients dropped, tx dropped → rx gets Disconnected → threads exit). OK actually hub is dropped when server thread exits, so SSE threads terminate. But api_stop joins handle — recv loop breaks within 400ms. Fine.

  16. **Panic tolerance**: json_ok has `.expect` on static header — fine. `handle_request` no catch_unwind; a panic in handler kills the server thread silently (loop breaks? no — panic in the thread kills thread; shutdown never set; ApiState still thinks running; api_status reports enabled but server dead). A panic in any handler → entire API dead silently while UI shows "已开启". No panic guard / no restart. That's a robustness architecture issue: P1. E.g. store.upsert Err is handled, but any unexpected panic (serde, etc.) kills thread. Actually panic would propagate and thread dies; `server` dropped → port closed. status shows enabled=true. Suggest catch_unwind around handle_request or monitor thread liveness in api_status. Good finding.

  17. **api_rotate_token while running**: calls api_stop then api_start — api_stop joins thread; but between stop and start, port released; SSE clients dropped without notice; also window where flag cleared — if api_start fails (port still in TIME_WAIT?), returns Err but flag cleared and service off — acceptable. Minor: rotate causes brief downtime + drops SSE without retry hint. P2.

  18. **log_line logs task title** (change op ... title=) — task content into api.log plaintext; privacy consideration; and log injection via title containing newline → forged log lines. Title 200 chars can contain \n → log injection. P2.

  19. **query_param repeated key takes first — fine. since parse<u64> fine.**

  20. **rate_check uses wall-clock now_ms; clock jump backwards → window never resets until 60s drift — minor.**

  21. **MAX_BODY check: `Ok(n) if n > MAX` — take(MAX+1) read_to_end — fine.**

  22. **verify_bearer: header field equiv case-insensitive — fine. Non-constant-time compare — P2.**

  23. **api_status calls load_or_create_token even when disabled** — creates token file even if user never enabled API. Minor.

  24. **Trailing-slash / path normalization**: `/api/tasks/` with empty id → strip_prefix gives "" → get_task with id "" → 404, fine. `/api/tasks/../tasks` — no normalization but id is just string compare, no traversal (id used only for equality compare, not file path). file_path is stored but not used by server. OK.

  25. **SSE thread leak on broadcast to disconnected**: retain removes failed senders only during broadcast; idle dead connections linger but harmless.

  Prioritize:

  - P0? Maybe none strictly P0. The unbounded channel + blocking write memory growth is the closest to P0 but requires slow consumer. I'd put: 
    - P1: single-threaded server + unbounded blocking body read (slowloris stalls entire API; no socket timeouts).
    - P1: SSE per-client unbounded mpsc + blocking write_all with no timeout → slow/dead client → unbounded memory + thread leak; no cap on SSE connection count.
    - P1: rate limiter before auth & global → unauthenticated local process can exhaust budget, DoS legit bot; /api/health exempt from limit.
    - P1: handler panic kills server thread silently; api_status still reports enabled (no liveness check / no catch_unwind).
    - P1 or P2: SSE hub only sees API-originated changes; UI/bot-side task changes never broadcast → external bot's realtime view silently incomplete (architecture: event source not unified at store layer).
    - P2: token compare non-constant-time; token file plaintext default perms.
    - P2: log injection via title/newline into api.log; log contains task content.
    - P2: SSE replay/connected race → duplicate events (needs client-side id dedupe; documented?).

  Difference from existing audits: AGENT-FLOW said "鉴权 + 限流 + SSE + 审计事件齐全 ✅" — my findings contradict/refine that. MANUAL P0-2 was about file splitting (done). So all new.

  Keep ≤ 50 lines. Let me verify the claim about tiny_http blocking read: `read_body_limited` uses req.as_reader().take().read_to_end — blocks until client closes body or sends Content-Length bytes. If client sends Content-Length: 1000000 but trickles, the single server thread blocks in read → entire API stalls. tiny_http doesn't set read timeout by default. Yes.

  Also health exempt from rate limit — combined with single thread, cheap though.

  Line count: aim ~40 lines.

• # Chunk 1 审查结果：API 层安全 + 边界

  ## 风险清单

  - [P1] [api_handlers.rs:391/521 + api_server.rs:116] **单线程事件循环 + 无 socket 读超时**：所有请求在唯一 recv 线程内同步处理，`read_body_limited` 的 `read_to_end` 无超时——一个声明了 `Content-Length` 但不发完 body 的连接（slowloris）即可永久卡死整个 API 服务，合法 bot 全部挂起。建议：给连接设 read timeout 或 body 读取移出主循环。
  - [P1] [api_server.rs:73 + api_handlers.rs:693-749] **SSE 每客户端无界 mpsc + 阻塞写无超时**：客户端连上但不读 → TCP 缓冲满 → writer 线程永久阻塞在 `write_all`，`tx.send` 继续无界排队 → 内存无上限增长；SSE 连接数也无上限（每连接一线程）。这是"开外部机器人后 app 越跑越慢/内存涨"的典型根因。建议：有界 channel（满则踢客户端）+ 写超时 + 连接数上限。
  - [P1] [api_handlers.rs:84-94] **限流在鉴权之前且全局共享**：无 token 的本地任意进程可用 401 请求耗尽 120/min 预算，合法 bot 被 429 拒绝（本地 DoS）；`/api/health` 同时免鉴权又免限流。建议：鉴权失败不计入（或独立小额度）预算。
  - [P1] [api_server.rs:112-133] **handler panic 即服务静默死亡**：`handle_request` 无 `catch_unwind`，任何 panic 杀死唯一服务线程，`ApiState` 仍 `Some`，设置页显示"已开启"但端口已死。建议：外层 catch_unwind + `api_status` 用 `handle.is_finished()` 报真实活性。
  - [P1] [api_server.rs:110 + api_handlers.rs:298] **EventHub 只挂在 API 写路径上**：UI 看板 / bot 工具侧的任务变更不经 `after_change`，`/api/events` 订阅者永远收不到——外部机器人的"实时视图"系统性不完整（架构上事件源应在 store 层统一，而非 handler 层）。这是"bot 和用户改的数据互相看不见"类 bug 的根因。
  - [P2] [api_auth.rs:72] token 比较用普通字符串相等（非恒定时间）；`api-token.txt` 明文落盘且未设 0600 权限。回环场景风险有限，但属纵深防御缺口。
  - [P2] [api_handlers.rs:299-305] 变更日志直接拼接 `task.title`：标题含 `\n` 可注入伪造日志行，且任务内容明文进 `api.log`（敏感信息落盘）。
  - [P2] [api_handlers.rs:713-733] SSE 连接建立与 replay 之间存在窗口：`last_id` 之后、replay 快照之前的事件会同时走历史重放和 live channel，客户端收到重复事件（协议层面需要求客户端按 `id:` 去重，目前未文档化）。

  ## 整体评估
  鉴权路径本身干净（统一 401、无绕过、无 token 回显），但**可用性/资源边界是薄弱环节**：单线程 + 无超时 + 无界队列的组合，使任何一个异常客户端都能拖死或拖胀整个服务——这正是"开了外部机器人就出怪 bug"的架构层土壤。

  ## 与已有审计差异
  重复 0 条，新发现 8 条（AGENT-FLOW 结论"鉴权+限流+SSE 齐全 ✅"只验证了功能存在，未覆盖上述资源边界与事件源完整性问题）。

To resume this session: kimi -r session_211bea1c-0717-4e20-9fcc-b37ce9b3bc0d

**风险清单**                                                       

​                                                            

  • **[P1] [api_server.rs:124-157] worker-per-request 无并发上限，A4 超时只解放主循环**：每个请求（含 401 洪泛、health 洪 

   泛）新建一个 OS 线程；15s 超时后主循环继续 accept，但 worker"续跑直到客户端断开"且 socket 无读超时——slowloris 从" 

   卡死单线程"变成"线程无限堆积"（每线程数 MB 栈），内存/线程耗尽。SSE 同样每连接一线程无上限。**建议**：worker 池/信号量

   上限 + socket 级读超时。                                              

  • **[P1] [api_handlers.rs:753-756 + api_server.rs:52] EventHub 随** **api_start** **重建，事件 id 归零**：TauriStore 每次启动新 

   建 EventHub，next_id 从 0 重计、history 清空；客户端持旧 since 重连后 replay 为空，且新事件 id 小于其       

   Last-Event-ID，标准去重逻辑会**静默丢弃全部新事件**——断线重放协议跨重启不可靠。**建议**：id 持久化，或重启时发 reset 事件/

   换 id 命名空间。                                                  

  • **[P2] [api_server.rs:183-188 + api_handlers.rs:795-807] recv_error 路径服务假活**：panic 已被 catch_unwind 覆盖，但  

   recv_timeout 返回 Err 时线程 break 退出，ApiState 仍是 Some，api_status 继续报 enabled=true（设置页显示"已开启"但 

   端口已死）。**建议**：api_status 加 handle.is_finished() 活性检查。                          

  • **[P2] [api_handlers.rs:811-829]** **api_rotate_token** **顺序错误导致三态不一致**：先覆写 token 文件再 stop/start；若 start  

   失败（端口 TIME_WAIT/占用），结果是服务已停 + flag 已清 + token 已换，且旧 SSE 客户端无提示断连。**建议**：先起新服务 

   成功再落新 token，或失败回滚。                                           

​                                                            

  **上轮遗留（已列过、未修复，仅更新状态）**                                        

​                                                            

  • **上轮 #5（SSE 事件源）仍未真正修复**：A5 把 hub 移进 store 层，但 TauriStore 仍是 api_start 私有实例，全仓仅     

   api_handlers.rs:281 调 notify_change——UI 看板（db_upsert 命令）和 bot 侧写入对 /api/events 订阅者依然不可见。   

  • 上轮 #6（token 非恒定时间比较 + 明文 0644）、#7（title 注入日志行）、#8（replay/connected 竞态重复）代码未动。   

  • 已修复：#1 单线程、#2 无界 mpsc（sync_channel 256）、#3 限流先于鉴权、#4 panic 杀服务。              

​                                                            

  **整体评估**                                                       

​                                                            

  A1-A5 修复方向正确但**只堵了上一轮的点名路径**：资源上限（线程数）、事件 id 跨重启连续性、状态活性上报这三个架构面仍是敞

  口，且 401 洪泛在新模型下转化为线程耗尽向量。                                    

​                                                            

  **与已有审计差异**                                                    

​                                                            

  重复 0 条，新发现 4 条（另有 4 条上轮遗留项状态更新，不计入新发现）。   
