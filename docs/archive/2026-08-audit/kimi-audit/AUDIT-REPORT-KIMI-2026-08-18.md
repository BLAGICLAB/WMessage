# WMessage × Kimi Code 架构审计完整报告（2026-08-18~19）

> **生成者**：小九（OpenClaw）
> **数据源**：8 块 Kimi Code 串行审计结果（`docs/kimi-audit/kimi-chunk-{1..8}-result.md`）
> **计划**：`docs/kimi-audit/AUDIT-PLAN-KIMI-2026-08-18.md`
> **触发**：老板 2026-08-18 17:49 + 2026-08-19 01:06 指令
> **校验**：与 4 份原始审计（`AUDIT-AGENT-CORE` / `AUDIT-AGENT-FLOW` / `AUDIT-FIX-PLAN` / `AUDIT-REPORT-MANUAL`）+ F-1~F-7 修复 plan 交叉

---

## 0. 元数据

- **范围**：`/Users/renshi/Projects/wmessage/` 仓库全栈（src-tauri Rust + src 前端）
- **方式**：1 次 Kimi 调用 = 1 个 chunk，**严格串行**，禁止并行、禁止 sub-agent / Explore / Task
- **时间**：2026-08-18 17:53 ~ 2026-08-19 01:14（8 块，共 ~7 小时，含 Kimi 套餐断流后等下个账单周期接续）
- **结果文件**：8 份 `kimi-chunk-N-result.md` + 本份合并报告 + `AUDIT-FIX-PLAN-KIMI-2026-08-18.md`（chunks 1-4 的修复 plan）

---

## 1. 8 块产出汇总

| # | 标题 | 核心文件 | 字节 | 新发现 | 严重度分布 |
|---|---|---|---|---|---|
| 1 | API 层安全 + 边界 | api_handlers.rs + api_server.rs + api_auth.rs | 22 KB | 8 | 5 P1 + 3 P2 |
| 2 | DB 层 + 迁移 | db.rs + migration.rs | 31 KB | 10 | **1 P0** + 4 P1 + 5 P2 |
| 3 | Python 沙箱 | bot_py.rs | 19 KB | 8 | 4 P1 + 4 P2 |
| 4 | Profile + 中间件 + 审计 | profile.rs + middleware.rs + audit.rs | 22 KB | 11 | 4 P1 + 7 P2 |
| 5 | 前端状态 + IPC 错误 | storage.ts + App.tsx + errorHandler.ts + 任务卡 | 4 KB | 11 | **2 P0** + 5 P1 + 4 P2 |
| 6 | 错误处理全栈 | error.rs + 全 Tauri command | 20 KB | 8 | **2 P0** + 3 P1 + 3 P2 |
| 7 | 资源生命周期 + 后台任务 | lib.rs + 全 Tauri command + bot_scheduler + bot_py | 27 KB | 5 | 2 P1 + 3 P2 |
| 8 | 跨平台打包 + 构建 | Cargo.toml + tauri.conf.json + build.rs + capabilities/ | 14 KB | 8 | **2 P0** + 2 P1 + 4 P2 |
| **合计** | | | **159 KB** | **69** | **7 P0 + 27 P1 + 35 P2** |

**重复 0 条**（Kimi 显式核对过 4 份原始审计 + F-1~F-7 + chunks 1-8 互不重叠）。

---

## 2. P0 阻塞清单（7 条 / 1 已修）

### ✅ 已修

| ID | 位置 | 描述 | 修复 commit |
|---|---|---|---|
| **P0-1** | `db.rs:576-602` + `db.rs:520-527` | `bot_history_save` / `bot_session_delete` 无事务 → 崩溃在 DELETE 与 INSERT 之间 → 整个会话聊天记录永久丢失 | ✅ `11e9ec9`（2026-08-18 18:54） |

### ❌ 未修（6 条，跨 4 个块）

#### 前端状态（chunk 5）

- **P0-5A** [`storage.ts:13-21` + `App.tsx:88-107`] **读失败伪装成空库，触发种子写入**：`db_load` 静默失败返回 `[]`，启动逻辑把"读失败"当"新库" → 写入 SEED（甚至跑 localStorage 迁移分支）→ 用户看到空板 + 种子卡，且种子真实落库。建议：区分 error 与 empty，失败时告警且不进种子/迁移分支。

- **P0-5B** [`App.tsx:147-150`] **mutate 落盘未 await 即广播**：挂件收广播立刻 `db_load` 可能读到提交前快照并回写旧数据 → UI/DB/挂件三方分叉。建议：await 落盘后再 emit。

#### 错误处理（chunk 6）

- **P0-6A** [`bot_chat.rs:306` + `:507`] **`Err("机器人聊天已关闭…".into())` 走 `From<&str>` → `INTERNAL`（recoverable=false）**，后端明明有专用变体 `BotDisabled` / `ApiKeyMissing`（recoverable=true）。前端 `hintForCode` 永远匹配不上，两个最高频用户错误拿到的是「可重试；如反复出现请反馈日志」的通用内部错误引导。建议：直接 `Err(CommandError::BotDisabled)` / `Err(CommandError::ApiKeyMissing)`。

- **P0-6B** [`errorHandler.ts:44,61,73`] **前端 hint 与后端 `is_recoverable()` 系统性矛盾**：`KEYRING_ERROR` / `UNKNOWN_TOOL` / `DB_ERROR` / `IO_ERROR` / `INTERNAL` 后端标 `recoverable=false`，hint 却写「可重试」；且 `recoverable` 字段只进 `console.error`，UI 从未消费（`error.rs` 注释承诺的「重试按钮」不存在）。老板看到的 bug 体感（"提示说可重试但重试没用"）正源于此。建议：以 `recoverable` 字段驱动提示/重试按钮，删除 hint 里的硬编码「可重试」。

#### 跨平台打包（chunk 8）

- **P0-8A** [`tauri.conf.json:28`] **`"targets": "all"` 触发 NSIS / WiX / AppImage 全家桶**：老板已实测 NSIS 跨平台崩溃（2026-08-14）；Windows 上 "all" 还会拉 WiX（MSI），Linux 上拉 AppImage（webkit2gtk 打包老大难）。任何一台机器打包都会撞没配好的 target → 打包失败被误判为代码 bug。建议：显式收窄，如 Windows `["nsis"]`、macOS `["dmg"]`、Linux `["deb"]`。

- **P0-8B** [`.cargo/config.toml:5`] **`rustc-wrapper` 硬编码绝对路径 `/Users/renshi/.cargo/bin/sccache`**：换机器 / CI / Windows 上 sccache 不存在时 `cargo build` 直接失败，且这是仓库级配置会随 git 分发。更糟的是**老板踩坑要求的 mingw-w64 linker 配置（`[target.x86_64-pc-windows-gnu]`）不在此处、也不在任何地方**——跨编译 Windows 必失败。建议：改为可移植写法（`rustc-wrapper = "sccache"` 走 PATH）或移出仓库；补 mingw-w64 linker 配置。

---

## 3. P1 修复批次（27 条，按主题归档 8 批）

### Batch A：API 资源安全（5 项）—— Chunk 1
**症状最稳定（外部机器人启用即触发）**

- **A1** [`api_handlers.rs:391/521` + `api_server.rs:116`] 单线程 + 无 socket 读超时 → slowloris 永久卡死整个 API
- **A2** [`api_server.rs:73` + `api_handlers.rs:693-749`] SSE 每客户端无界 mpsc + 阻塞写无超时 → 内存无上限增长 + 写阻塞 → 「开了外部机器人越跑越慢」
- **A3** [`api_handlers.rs:84-94`] 限流在鉴权前 + 全局共享 → 无 token 本地 DoS
- **A4** [`api_server.rs:112-133`] handler panic 即服务静默死亡 + `api_status` 不报真实活性
- **A5** [`api_server.rs:110` + `api_handlers.rs:298`] EventHub 只挂在 API 写路径 → 外部机器人永远看不到 UI / bot 工具侧变更（事件源应统一在 store 层）

### Batch B：DB 事务 / 一致性（4 项）—— Chunk 2
**与 P0-1 同一文件，一起做**

- **B1** [`migration.rs:405-415` + `462-464`] 文件移动与 DB 更新非原子 → 附件链接永久解绑
- **B2** [`migration.rs:309` + `db.rs:635-644`] 读快照 → 长文件操作 → 全行覆盖写 → lost update
- **B3** [`db.rs:806-1033` + `migration.rs:662`] 全部 Tauri 命令 sync fn → 主线程阻塞 IO（迁移拷大目录冻结算分钟级）
- **B4** [`db.rs:961-968, 1013-1020`] `db_merge` / `tasks_import` `r.get::<i64>` 在 NULL updated_at 上抛 `InvalidColumnType` → 整个导入失败

### Batch C：Python 沙箱熔断（4 项）—— Chunk 3
**触发条件是 LLM 行为，但已可复现**

- **C1** [`bot_py.rs:259-282`] 超时只覆盖主进程 → 孙进程继承管道 → `rx.iter()` 永久阻塞 → `run_python` 永久挂死
- **C2** [`bot_py.rs:198-208` + `792-806`] 无内存/CPU 限额 + `timeout_secs` 无上限钳制 → 跑 Python 系统卡死
- **C3** [`bot_py.rs:806` + `849/881` 等] 审计在最危险的路径上缺席（超时/失败不记审计）
- **C4** [`bot_py.rs:792-806`] `py_exec_sync` 同步阻塞跑在 async runtime 线程上（注释自证）

### Batch D：Profile / 中间件 / 审计（4 项）—— Chunk 4

- **D1** [`middleware.rs:123-126`] IntentRouterMiddleware 恒返回 `Some` → pre_step 链永远短路 → 未来中间件全死代码
- **D2** [`middleware.rs:98-111`] 安全闸门 fail-open → AtomicGuard 一次误操作就被旁路
- **D3** [`profile.rs:85-88, 173-183`] profile 非原子写 + 静默重置 → 用户资料消失
- **D4** [`profile.rs:173/233/268`] profile read-modify-write 无锁 → 主窗 + 挂件并发丢更新

### Batch E：前端状态层（5 项）—— Chunk 5

- **E1** [`storage.ts:24-41, 81-98`] 全部写路径静默吞错 → UI/DB 永久分叉
- **E2** [`App.tsx:188-197`] 事件合并路径的规则改动不落盘 → 三端长期不一致
- **E3** [`App.tsx:282-286`] 导入后重读失败 → 清空 UI 并广播 → 制造"数据全丢"假象
- **E4** [`TaskCardContent.tsx:92-95`] Escape 取消后 blur 仍提交草稿
- **E5** [`App.tsx:147-150`] mutate 落盘未 await 即广播（**P0-5B** 同根）

### Batch F：错误处理 / CommandError 迁移残留（3 项）—— Chunk 6

- **F1** [`bot.rs:95`] `read_api_key` 走 `String` 而非 `KeyringError` → 同类故障产出两种 code + `has_api_key` 把所有错吞成 `false` 制造"未配置"假象
- **F2** [`bot_py.rs:48,829,872,892,931,948,967` + `lib.rs:30`] 8 个 `#[tauri::command]` 仍是 `Result<T, String>`（bot_py 7 + `copy_file_with_title`）
- **F3** [`bot.rs:241` / `migration.rs:983`] `bot_log_read` / `migration_log_read` 把读文件错吞成「暂无日志」

### Batch G：资源生命周期（2 项）—— Chunk 7

- **G1** [`api_handlers.rs:791` + `:691`] `api_stop` / `api_rotate_token` 只 join accept 线程，SSE writer 线程不被追踪/不被通知 → 旧 hub `tx` 永不 drop → writer 永久循环发 keepalive → 旧客户端以为活着却永远收不到新事件；每次 rotate 漏 N 个线程
- **G2** [`bot_py.rs:259-282` + `bot.rs:327`] `run_python` 读阶段无整体兜底超时 → 与 **C1** 同根（孙进程继承管道永久挂死，且阻塞 tokio worker 最长 120s，`StopGuard` 无法中断）

### Batch H：跨平台打包配置（2 项）—— Chunk 8

- **H1** [`tauri.conf.json` 全文] 无 `bundle.resources`、无 `bundle.windows` 节 → `WebView2Loader.dll` 未声明为资源 → `cargo build`（而非 `tauri build`）出便携包时该 dll 不会被复制到 exe 旁（老板 2026-08-14 实测坑在此配置层面无任何防护）
- **H2** [`tauri.conf.json` 全文] 无 `bundle.macOS` 节（signingIdentity / hardenedRuntime / entitlements / notarize）→ 本地 dev 正常，`tauri build` 出的 `.app`/`.dmg` 是 ad-hoc 签名，拷到别的 Mac 被 Gatekeeper 报"已损坏"；配合 `macOSPrivateApi: true`（lib.rs 透明度需要）永久关上 App Store 的门

---

## 4. P2 长期清单（35 条，按文件归档）

### `api_handlers.rs` / `api_auth.rs`（3 项）
- P2-1 token 比较非恒定时间 + `api-token.txt` 明文无 0600
- P2-2 变更日志直接拼接 `task.title`（含 `\n` 注入 + 任务内容明文进 api.log）
- P2-3 SSE replay race → 客户端收到重复事件（需协议层文档化按 id 去重）

### `db.rs` / `migration.rs`（5 项）
- P2-4 便携模式首次拷贝老库：`wal_checkpoint` 错误被 `let _` 吞掉 + 只拷 `.db` 不拷 `-wal`（WAL 里的写入静默丢失）
- P2-5 `workspace_upsert` / `workspace_delete` 循环 execute 无事务
- P2-6 `conflict_free_name` TOCTOU：`fs::rename` 在 Unix 默认覆盖已存在目标
- P2-7 `migrate_data_json` 触发条件仅为 `count==0`（删任务后重启复活 + 便携模式 data.json 不迁移）
- P2-8 `save_rules` 直接 `fs::write` 覆盖（崩溃 → 规则静默丢失）

### `bot_py.rs`（4 项）
- P2-9 spawn 失败路径泄漏临时目录（无启动清扫机制）
- P2-10 `detect_python()` 每次重新探测（最多 3 次进程 spawn，无缓存）
- P2-11 审计日志注入：`truncate_for_log` 不剥换行（与 P2-2 同类，不同 sink）
- P2-12 `run_python` 并发无闸门

### `middleware.rs` / `audit.rs`（4 项）
- P2-13 middleware `dyn Middleware` 无 `catch_unwind`（panic 跳过 tool.return 审计）
- P2-14 middleware 双 Vec 分离注册（漏注一边静默半生效）
- P2-15 审计写失败全静默（open fail / writeln / rotate rename 全吞；每事件做磁盘写探测）
- P2-16 kv 值不转义（`truncate_for_log` 不剥 `\n` / `| ` → 注入伪造 INFO | 前缀）

### `profile.rs`（3 项）
- P2-17 读头像无大小上限（写入端限 5MB，读取端无限）
- P2-18 先删旧头像再拷贝（copy 失败 → 头像静默丢失）
- P2-19 目录解析逻辑两处拷贝（profile.rs:55 vs db.rs:50，drift 风险）

### 前端（4 项）—— Chunk 5/6
- P2-20 `storage.ts:116-117` + `App.tsx:144-146` 全量重排刷新所有 `updatedAt`，破坏导入合并语义
- P2-21 `App.tsx:144-146` mutate 直接改写 state 对象字段
- P2-22 `TrashPage.tsx:18` + `ArchivePage.tsx:25` 回收站未排序 + 软删除不清调度字段
- P2-23 `TaskCardContent.tsx:12-14, 67-69` 双实现漂移风险 + useEffect 依赖缺失

### `bot_py.rs` / `lib.rs`（3 项）—— Chunk 7
- P2-24 `lib.rs:393-401` `RunEvent::ExitRequested` 只销毁主窗口，不 api_stop、不杀 Python 子进程（孤儿）
- P2-25 `bot_py.rs:233` spawn 失败路径即时泄漏临时目录（P2-9 补充）
- P2-26 `lib.rs:57` `bring_main_to_front` 主线程 `sleep(80ms)` 冻结 UI

### `error.rs` / `bot_chat.rs` / 跨平台（3 项）—— Chunk 6/8
- P2-27 `error.rs` 整体 无平台差异分层 + 无后端落盘/聚合埋点（致命错误无 alerting）
- P2-28 `bot_chat.rs:633,643,646` 业务状态拒绝全部降级为 `INTERNAL`，缺 `TaskInvalidState` 变体
- P2-29 `tauri.conf.json:4` vs `Cargo.toml:3` vs `package.json:4` 版本号 4 处漂移（1.0.0 / 0.1.0 / 0.1.0 / 便携包 1.0.1）

### `capabilities/default.json` / Cargo deps（3 项）—— Chunk 8
- P2-30 `capabilities/default.json:20-27` `opener:allow-open-path` 路径通配 `**`（无白名单兜底）
- P2-31 `lib.rs:286` 托盘仅 `cfg(target_os = "windows")` → Linux 无托盘 + `copy_file_with_title` Linux 直接 `Err`（属显式降级但未实测）
- P2-32 `Cargo.toml:38` keyring 在 Linux 走 secret-service（需 dbus），无 target 条件、无运行时探测

### WidgetApp / IPC 错误可见性（3 项）—— Chunk 6
- P2-33 `WidgetApp.tsx:168,171,195,198` 5 秒轮询 `load().catch(() => {})` 永久静默
- P2-34 全仓 `.catch(() => {})` ~25 处，其中 emit/startDragging 类可接受，db_load/loadWorkspace 类不可接受（数据加载类 catch 应有降级）
- P2-35 `errorHandler.ts` 前端错误分级 `recoverable` 字段未消费 + 空 msg 静默（无 alert）

---

## 5. 重点产出（4 大类）

### 5.1 P0 阻塞（老板感知的"基础功能挂掉"类）
**7 条 P0，1 已修 6 未修**，按主题：
- **数据丢失**（1）：P0-1 已修（聊天记录事务包裹）
- **状态分叉**（2）：P0-5A（读失败伪装空库触发种子写入）+ P0-5B（mutate 未 await 即广播）
- **错误引导系统性误导**（2）：P0-6A（From<String> 绕过专用变体）+ P0-6B（前端 hint 与 recoverable 矛盾 + 字段未消费）
- **跨平台打包不能跨机器**（2）：P0-8A（targets:"all" 全家桶）+ P0-8B（rustc-wrapper 绝对路径 + 缺 mingw-w64 linker 配置）

### 5.2 隐藏安全（不主动触发看不出来）
- **A3 限流在鉴权前**（无 token 本地 DoS）
- **D2 安全闸门 fail-open**（AtomicGuard 一次误操作就被旁路）
- **A5 / E2 / B2 事件源不统一**（外部机器人 / 挂件 / 前端互不可见）
- **P2-30 opener:allow-open-path 路径通配**（bot prompt 注入诱导打开任意路径）
- **F1 read_api_key 走 String** + `has_api_key` 吞错 → macOS 钥匙串锁定时设置页显示"未配置"，用户反复填 key 仍失败且无提示
- **P2-1 token 非恒定时间比较 + 明文 0644**
- **P2-2 / P2-11 日志注入**（标题含 `\n` 伪造日志行）

### 5.3 隐式时序（最难复现的 bug 根因）
- **C1 / G2 run_python 孙进程继承管道** → `os.system("nohup xxx &")` 1 秒后主进程退出，读线程永久阻塞，`rx.iter()` 等不到 EOF → 整个工具链挂死
- **G1 api_stop SSE writer 线程泄漏** → 旧 hub `tx` 永不 drop → 旧客户端以为活着永收不到新事件
- **B2 全行覆盖 upsert + 无版本守卫** → 迁移拷大目录期间用户改的任务被旧快照覆盖（lost update）
- **P0-5B mutate fire-and-forget** → 挂件读到的可能是提交前快照
- **C3 / P2-15 审计在失败路径缺席** → 致命事件零痕迹
- **P2-24 ExitRequested 无清理** → Python 子进程变孤儿（配合 C1 可永久残留）

### 5.4 架构层 bug 根因（系统性而非偶发）

| 根因 | 触发的具体 bug | 涉及 findings |
|---|---|---|
| **错误处理：从 String 逃生舱系统性地绕过 CommandError 设计目标** | 提示误导 / 重试无效 / 设置页灵异 / 关键错误被吞 | P0-6A/B + F1/F2/F3 + P2-27/28 |
| **资源生命周期：SSE / 子进程 与 API/UI 退出钩子脱钩** | 外部机器人莫名卡 / Python 挂死 / 内存涨 / 子进程孤儿 | A2 + C1/C2 + G1/G2 + P2-24/25 |
| **跨平台打包：所有跨平台知识都在文档/人脑里，配置零沉淀** | 换机器打包必翻车 / Windows 跨编译必失败 / Mac Gatekeeper 拒签 | P0-8A/B + H1/H2 + P2-29/30/31/32 |
| **数据层事务边界只覆盖了一半写路径** | 聊天记录丢 / 附件链接断 / 跨窗口丢更新 / 迁移误覆盖 | P0-1 + B1/B2 + D3/D4 + P2-4/5/8 |
| **审计 / 错误处理把"静默"当默认** | 失败不可见 / 致命事件无 alerting / 监控盲区 | C3 + D2 + E1 + F3 + P2-15/16/33/34 |
| **中间件抽象先于需求**（可插拔但永远短路在第一个） | 未来中间件加一个死一个 / 安全闸门形同虚设 | D1 + P2-13/14 |

---

## 6. 已修

| ID | 描述 | commit |
|---|---|---|
| **P0-1** | `bot_history_save` / `bot_session_delete` 事务包裹（220 passed + 3 新测） | `11e9ec9` |

---

## 7. 待办（按优先级排期）

### 本周内（建议）
- **P0-5A/B** 前端读失败伪装 + mutate 未 await（单点修，半天）
- **P0-6A/B** 错误处理 From<String> 收口 + recoverable 字段驱动 UI（半天）
- **P0-8A/B** 跨平台配置收窄 targets + rustc-wrapper 可移植 + 补 mingw-w64 linker（半天）

### 下周
- **Batch A**（API 资源安全 5 项，~3.5 工日）
- **Batch B**（DB 事务/一致性 4 项，~4 工日）
- **Batch C**（Python 沙箱 4 项，~3.5 工日）

### 第三周
- **Batch D / E / F**（中间件/profile/前端/错误处理迁移，~5 工日）
- **Batch G / H**（资源生命周期 + 跨平台打包配置，~3 工日）

### 长期收尾
- **P2 35 项** 合并式修复（~5 工日；横切关注点同步做可省 2-3 工日）
- **跨主题横切**：
  1. 统一原子写（profile.rs / save_rules / db.rs 各处 `fs::write` → `atomic_write()` helper）
  2. 统一数据目录解析（profile.rs:55 vs db.rs:50 → 单一 `data_dir()`）
  3. 统一审计转义（`truncate_for_log` → `escape_for_log`）
  4. 统一 Mutex 模式（profile + DB write 进程内 Mutex）

**总额评估**：~18.5 工日 → **真实可压 13-15 工日 ≈ 3 周**

---

## 8. 文件索引

| 文件 | 用途 |
|---|---|
| `kimi-chunk-{1..8}-result.md` | 8 份原始 Kimi 审计输出（含思考过程） |
| `AUDIT-PLAN-KIMI-2026-08-18.md` | 8 块切分 + 串行执行策略 + 单 chunk prompt 模板 |
| `AUDIT-FIX-PLAN-KIMI-2026-08-18.md` | chunks 1-4 的修复 plan（含 P0-1 修复说明） |
| **本文件** `AUDIT-REPORT-KIMI-2026-08-18.md` | 8 块合并去重后的完整报告（重点产出 + 排期） |
| `../AUDIT-AGENT-CORE-2026-08-18.md` | 4 份原始审计之一：Agent 内核编排 |
| `../AUDIT-AGENT-FLOW-2026-08-18.md` | 4 份原始审计之二：端到端 6 流程 trace |
| `../AUDIT-FIX-PLAN-2026-08-18.md` | 4 份原始审计之三：F-1~F-7 修复 plan |
| `../AUDIT-REPORT-MANUAL-2026-08-18.md` | 4 份原始审计之四：广角扫描 |

---

_报告生成时间：2026-08-19 01:20_
_生成者：小九（OpenClaw / MiniMax-M3）_