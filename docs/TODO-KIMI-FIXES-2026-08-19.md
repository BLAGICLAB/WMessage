# WMessage × Kimi Code 修复验收清单（2026-08-19）

> **目的**：按老板 2026-08-19 07:25 指令「按顺序逐个解决，commit，勾清单」
> **生成者**：小九（OpenClaw / MiniMax-M3）
> **覆盖**：Batch C 原 4 项 + P2-9..12 / NEW-C-3..7 / NEW-B-2..6 / Batch E / NEW-D-1..6
> **修复策略**：每 Phase 一个 Kimi code 调用，串行（套餐少，禁并行）；每 Phase 结束勾对应项并 commit
> **基线**：cargo test --lib 237 pass / 2 pre-existing fail（C 路径 DSL 迁移遗留，不在范围）

---

## Phase 1 — Batch C 原 4 项 + P2-9..12（Python 沙箱熔断）⭐ 老板最容易撞

老板体感：**C1 `run_python` 永久挂死（孙进程继承管道）+ C2 跑 Python 系统卡死**。约 2-3 工日。

- [x] **C1** `bot_py.rs:259-282` — 孙进程继承管道 → `rx.iter()` 永久阻塞 → run_python 永久挂死
- [x] **C2** `bot_py.rs:198-208 + 792-806` — 无内存/CPU 限额 + `timeout_secs` 无上限钳制 → 跑 Python 系统卡死
- [ ] **C3** `bot_py.rs:806 + 849/881` — 超时/失败路径不记审计
- [ ] **C4** `bot_py.rs:792-806` + `bot.rs:1327` — `py_exec_sync` 同步阻塞跑在 async runtime
- [ ] **P2-9** `bot_py.rs:234` — spawn 失败路径泄漏临时目录（无启动清扫机制）
- [ ] **P2-10** `bot_py.rs:205` — `detect_python()` 每次重新探测（最多 3 次进程 spawn，无缓存）
- [ ] **P2-11** `bot_py.rs:1053` — `truncate_for_log` 不剥换行
- [ ] **P2-12** 全仓 — `run_python` 并发无闸门

---

## Phase 2 — NEW-C-3..7（Batch C 重审新发现 P2）

约 1 工日。

- [ ] **NEW-C-3** `bot_py.rs:261` — `try_wait` 错误路径 `?` 直返：子进程成孤儿 + 临时目录泄漏 + 无审计
- [ ] **NEW-C-4** `bot_model_loop/bot_slash StopGuard` — `/stop` 不覆盖在途 Python 子进程（只能等 60-120s 超时）
- [ ] **NEW-C-5** `bot_py.rs:245/253` — 输出 >64KB → reader 关管道 → 子进程被 SIGPIPE 静默杀，`exit_code=None` 无说明
- [ ] **NEW-C-6** `audit.rs:82 + bot_py.rs:849` — `write_event` kv 值 + `doc_extract` path 审计行不剥换行（与 P2-11 同族不同 sink）
- [ ] **NEW-C-7** `bot_py.rs:1043` — `trim_end_matches(".{ext}")` 大小写敏感 → "周报.DOCX.docx"

---

## Phase 3 — NEW-B-2..6（Batch B 重审新发现 P2）

约 2 工日。

- [ ] **NEW-B-2** `migration.rs journal_*` — journal 三次写各开独立 open_db 且不持写锁，靠 2s busy_timeout 兜底
- [ ] **NEW-B-3** `db.rs / migration.rs` — B3 残留 sync 命令：workspace_*、bot_session_create/rename、migration_log_read（主线程读最大 5MB）
- [ ] **NEW-B-4** `db.rs:270 RESET_ONCE` — 清 bot_assigned 的 UPDATE 吞错 + Once 失败不重试 → 残留 🤖 到下次重启
- [ ] **NEW-B-5** `migration.rs move_entry 跨卷回退` — copy 成功 remove 持续失败时每轮生成新冲突名再 copy → 归档副本累积
- [ ] **NEW-B-6** `db.rs tasks_export` — `fs::write` 非原子（与 P2-8 同类），崩溃留半截 JSON

---

## Phase 4 — Batch E（前端 E1-E5）

约 1.5 工日。E5 已并入 P0-5B（commit 9d8d7c8），跳过。

- [ ] **E1** `src/storage.ts:24-41, 81-98` — 全部写路径静默吞错 → UI/DB 永久分叉
- [ ] **E2** `src/App.tsx:188-197` — 事件合并路径规则改动不落盘 → 三端长期不一致
- [ ] **E3** `src/App.tsx:282-286` — 导入后重读失败 → 清空 UI 并广播 → 制造"数据全丢"假象
- [ ] **E4** `src/components/TaskCardContent.tsx:92-95` — Escape 取消后 blur 仍提交草稿
- [x] **E5** ~~mutate 落盘未 await 即广播（已并入 P0-5B，commit 9d8d7c8）~~

---

## Phase 5 — NEW-D-1..6（中间件/profile/audit P2）

约 2 工日。其中 NEW-D-5/6 顺手把 P2-15/P2-16 部分一起收拾。

- [ ] **NEW-D-1** `bot.rs:282-307 vs 343` — `pre_execute` 拦截 + `skill_on_step` 报错两条早退路径发了 `tool.call` 永不发 `tool.return` → 统计面板「悬挂调用」；skill_on_step 错误路径连 Warn 都没有
- [ ] **NEW-D-2** `profile.rs:247-251` — `set_avatar` save 失败回滚 `remove_file(dest)` 在同扩展名覆盖场景误删在役头像（P2-18 未覆盖的反向回退）
- [ ] **NEW-D-3** `profile.rs:274-277` — `remove_avatar` 先删文件后 save，save 失败 → 磁盘 json 悬挂引用已删文件
- [ ] **NEW-D-4** `profile.rs:106-115` — `entry_view` 不校验 avatar 字段是否纯文件名，手改 `profile.json` 可路径穿越读任意文件并 base64 广播
- [ ] **NEW-D-5** `audit.rs:52-60 vs 80-84` — `format_event_line` 是生产 `write_event` 行拼装的测试专用拷贝，drift 时测试照样绿
- [ ] **NEW-D-6** `middleware.rs:17/98/106` — helper 写死 Wry `AppHandle`，与文件头「泛型 Runtime」注释自相矛盾；D2 fail-open 回退无法单测

---

## Phase 6 — 待办（Phase 1-5 完成后）

约 4-5 工日。

### Batch D 原 D1-D4
- [ ] **D1** `middleware.rs:123-126` — IntentRouterMiddleware 恒返回 `Some` → pre_step 链永远短路 → 未来中间件全死代码
- [ ] **D2** `middleware.rs:98-111` — 安全闸门 fail-open → AtomicGuard 一次误操作就被旁路
- [ ] **D3** `profile.rs:85-88,173-183` — profile 非原子写 + 静默重置 → 用户资料消失
- [ ] **D4** `profile.rs:173/233/268` — profile read-modify-write 无锁 → 主窗 + 挂件并发丢更新

### Batch F F1-F3
- [ ] **F1** `bot.rs:95` — `read_api_key` 走 `String` 而非 `KeyringError` → 同类故障产出两种 code + `has_api_key` 把所有错吞成 `false`
- [ ] **F2** `bot_py.rs:48,829,872,892,931,948,967` + `lib.rs:30` — 8 个 `#[tauri::command]` 仍是 `Result<T, String>`
- [ ] **F3** `bot.rs:241` / `migration.rs:983` — `bot_log_read` / `migration_log_read` 把读文件错吞成「暂无日志」

### Batch G G1-G2
- [ ] **G1** `api_handlers.rs:791 + :691` — `api_stop` / `api_rotate_token` 只 join accept 线程，SSE writer 线程不被追踪/不被通知
- [ ] **G2** `bot_py.rs:259-282 + bot.rs:327` — `run_python` 读阶段无整体兜底超时（与 C1 同根）

### Batch H H1-H2
- [ ] **H1** `tauri.conf.json 全文` — 无 `bundle.resources`、无 `bundle.windows` 节 → `WebView2Loader.dll` 未声明为资源
- [ ] **H2** `tauri.conf.json 全文` — 无 `bundle.macOS` 节（signingIdentity / hardenedRuntime / entitlements）

### 原 P2 35 项（已并入除外）

详见 `docs/kimi-audit/AUDIT-REPORT-KIMI-2026-08-18.md` §4。

---

## 进度

- Phase 1: 0/8
- Phase 2: 0/5
- Phase 3: 0/5
- Phase 4: 0/4 (E5 已勾)
- Phase 5: 0/6
- Phase 6: 0/11+

---

## 已勾选 / 已修复历史

- [x] **P0-1** bot_history_save 事务包裹 — commit `11e9ec9`
- [x] **P0-5A** 读失败伪装成空库 — commit `9d8d7c8`
- [x] **P0-5B** mutate 落盘未 await 即广播 — commit `9d8d7c8`
- [x] **P0-6A** `Err("...".into())` 走 `From<&str>` → `INTERNAL` — commit `b5bb01e`
- [x] **P0-6B** 前端 hint 与 recoverable 矛盾 — commit `b5bb01e`
- [x] **P0-8A** `"targets": "all"` — commit `0e699c0`
- [x] **P0-8B** rustc-wrapper 硬编码 + 缺 mingw linker — commit `0e699c0`
- [x] **Batch A**（A1-A5 + A6 bonus）5 项 — commits `f621157` `cc89ed8` `26a8169` `3187315` `1fa9006`
- [x] **B1** migration journal — commit `9a52272`
- [x] **B2** upsert WHERE 守卫 — commit `ae17174`
- [x] **B3** async + spawn_blocking — commits `02a3ecc` `33910c9`
- [x] **B4** NULL updated_at 兼容 — commit `84d007a`
- [x] **NEW-B-1** journal + 解绑对账 + replay 防覆盖 — commit `d097e46`
- [x] **NEW-C-1** doc_* 6 个 async 命令 spawn_blocking — commit `1c9b954`
- [x] **NEW-C-2** py_audit 补 BOT_LOG_LOCK — commit `885cd01`