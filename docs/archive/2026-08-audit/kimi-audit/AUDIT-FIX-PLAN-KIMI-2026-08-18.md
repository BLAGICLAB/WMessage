# WMessage × Kimi 架构审计修复计划（2026-08-18 全量）

> **生成者**：小九（OpenClaw）
> **数据源**：chunk 1-8 Kimi 审计结果（8 块 69 条新发现 / 0 重复）
> **前序**：P0-1 已修（commit `11e9ec9`，18:54，220→226 passed）
> **当前**：2026-08-19 06:08，老板指令「继续让 Kimi code 修复」
> **遗留**：2 个 pre-existing test fail（C 路径 DSL 迁移遗留，`target/debug/skills/design-style-skill` 无 step），**不在本 P0 修复范围**

---

## 1. 严重度分布（全 8 块）

| 严重度 | 数量 | 状态 |
|---|---|---|
| **P0** | 7 | 1 已修 / **6 未修（全部阻断 release）** |
| **P1** | 27 | 0 修 |
| **P2** | 35 | 0 修 |
| **合计** | **69** | — |

---

## 2. P0 必修（6 项，按"修一处堵一类"顺序）

> 全部阻塞 release，按修复互不依赖的拓扑排序。

### Batch P0-α：前端状态分叉（2 项）—— Chunk 5
**根因**：前端写路径 fire-and-forget + 读路径静默吞错 → UI/DB/挂件三方永久分叉。

| ID | 位置 | 症状 | 修复策略 |
|---|---|---|---|
| **P0-5A** | `src/storage.ts:13-21` + `src/App.tsx:88-107` | `loadTasksFromDb` catch 后返回 `[]`，启动逻辑把"读失败"当"空库" → 触发 localStorage 迁移或 SEED 写入 → 真实数据被静默覆盖 | 区分 `error` 与 `empty`；读失败时**不进** seed/migration 分支，必须显式告警。可选：返回 `Result<Task[], Error>` 或抛 `throw` 出去给调用方 |
| **P0-5B** | `src/App.tsx:147-150` | `upsertTasks(upserts); deleteTaskRows(deletes); setTasks(next); emit("tasks-changed");` — 前两个异步写未 await 即广播；挂件收到后 `db_load` 可能读到提交前快照并回写旧数据 | 改 async 函数 + `await` 两边写 + `await emit`；加单测覆盖顺序 |

**工期**：0.5 工日 / 2 项

### Batch P0-β：错误处理系统性误导（2 项）—— Chunk 6
**根因**：`Err("...".into())` 走 `From<&str>` → `INTERNAL`（recoverable=false），最高频用户错误被错配；前端 hint 与后端 `recoverable` 字段系统性矛盾。

| ID | 位置 | 症状 | 修复策略 |
|---|---|---|---|
| **P0-6A** | `src-tauri/src/bot_chat.rs:306` + `:507` | `Err("机器人聊天已关闭...".into())` 和 `Err("机器人 API 未配置...".into())` → `INTERNAL`，前端 `hintForCode` 永远匹配不到，提示变成"可重试；如反复出现请反馈日志" | 改用 `Err(CommandError::BotDisabled)` / `Err(CommandError::ApiKeyMissing)`（已存在的变体，recoverable=true）。同步扫一遍 `bot_chat.rs` `bot.rs` `migration.rs` `bot_py.rs` 所有 `Err("...".into())`，能映射到现有变体的全替 |
| **P0-6B** | `src/lib/errorHandler.ts:44,61,73` + 整个前端 hint 系统 | `KEYRING_ERROR` / `UNKNOWN_TOOL` / `DB_ERROR` / `IO_ERROR` / `INTERNAL` 后端标 `recoverable=false`，hint 仍写"可重试"；`recoverable` 字段从不被 UI 消费（error.rs 注释承诺的"重试按钮"不存在） | 以 `recoverable` 字段驱动提示/重试按钮；删除 hint 里的硬编码"可重试"；UI 侧真正消费 `recoverable` 显示重试按钮 |

**工期**：0.5 工日 / 2 项

### Batch P0-γ：跨平台打包（2 项）—— Chunk 8
**根因**：跨平台打包知识全部沉淀在文档/人脑里，配置文件零防护 → 换机器/CI/Windows 必翻车。

| ID | 位置 | 症状 | 修复策略 |
|---|---|---|---|
| **P0-8A** | `src-tauri/tauri.conf.json:28` | `"targets": "all"` 触发 NSIS / WiX / AppImage 全家桶；Mac 上 `makensis` 跨平台崩溃（老板 2026-08-14 实测） | 显式收窄：Windows `["nsis"]`、macOS `["dmg"]`、Linux `["deb"]`；后续如需 AppImage/NSIS 再加 |
| **P0-8B** | `.cargo/config.toml:5` + 缺失 | `rustc-wrapper = "/Users/renshi/.cargo/bin/sccache"` 硬编码绝对路径，换机器/CI 必失败；**mingw-w64 linker 配置（`[target.x86_64-pc-windows-gnu]`）仓库里完全没有**，跨编译 Windows 必失败 | 1. `rustc-wrapper = "sccache"` 走 PATH（fallback：无 sccache 直接 build）；2. 补 `[target.x86_64-pc-windows-gnu]` 配置（linker / ar / 必要 env） |

**工期**：0.5 工日 / 2 项

**P0 总工期**：~1.5 工日（**release 必过线**）

---

## 3. P1 修复批次（27 项，按主题归档 8 批）

### Batch A：API 资源安全（5 项）—— Chunk 1
**症状最稳定**（外部机器人启用即触发）

- **A1** 单线程 + 无 socket 读超时 → slowloris 永久卡死整个 API
- **A2** SSE 每客户端无界 mpsc + 阻塞写无超时 → 内存无上限增长 + 写阻塞 → 「开了外部机器人越跑越慢」
- **A3** 限流在鉴权前 + 全局共享 → 无 token 本地 DoS
- **A4** handler panic 即服务静默死亡 + `api_status` 不报真实活性
- **A5** EventHub 只挂在 API 写路径 → 外部机器人永远看不到 UI/bot 工具侧变更

**工期**：~3.5 工日

### Batch B：DB 事务 / 一致性（4 项）—— Chunk 2
**与 P0-1 同一文件，一起做**

- **B1** 文件移动与 DB 更新非原子 → 附件链接永久解绑
- **B2** 读快照 → 长文件操作 → 全行覆盖写 → lost update
- **B3** 全部 Tauri 命令 sync fn → 主线程阻塞 IO
- **B4** `db_merge` / `tasks_import` `r.get::<i64>` 在 NULL updated_at 上抛 → 整个导入失败

**工期**：~4 工日

### Batch C：Python 沙箱熔断（4 项）—— Chunk 3
**触发条件是 LLM 行为，但已可复现**

- **C1** 超时只覆盖主进程 → 孙进程继承管道 → `rx.iter()` 永久阻塞
- **C2** 无内存/CPU 限额 + `timeout_secs` 无上限钳制
- **C3** 审计在最危险的路径上缺席（超时/失败不记审计）
- **C4** `py_exec_sync` 同步阻塞跑在 async runtime

**工期**：~3.5 工日

### Batch D：Profile / 中间件 / 审计（4 项）—— Chunk 4

- **D1** IntentRouterMiddleware 恒返回 `Some` → pre_step 链永远短路 → 未来中间件全死代码
- **D2** 安全闸门 fail-open → AtomicGuard 一次误操作就被旁路
- **D3** profile 非原子写 + 静默重置 → 用户资料消失
- **D4** profile read-modify-write 无锁 → 主窗 + 挂件并发丢更新

**工期**：~2 工日

### Batch E：前端状态层（5 项）—— Chunk 5
**P0-5A/B 修完后剩**

- **E1** 全部写路径静默吞错 → UI/DB 永久分叉
- **E2** 事件合并路径的规则改动不落盘 → 三端长期不一致
- **E3** 导入后重读失败 → 清空 UI 并广播 → 制造"数据全丢"假象
- **E4** Escape 取消后 blur 仍提交草稿
- **E5** mutate 落盘未 await 即广播（同根 **P0-5B** 已覆盖，E5 可合并）

**工期**：~1.5 工日

### Batch F：错误处理 / CommandError 迁移残留（3 项）—— Chunk 6
**P0-6A/B 修完后剩**

- **F1** `read_api_key` 走 `String` 而非 `KeyringError` → 同类故障产出两种 code + `has_api_key` 把所有错吞成 `false` 制造"未配置"假象
- **F2** 8 个 `#[tauri::command]` 仍是 `Result<T, String>`（bot_py 7 + `copy_file_with_title`）
- **F3** `bot_log_read` / `migration_log_read` 把读文件错吞成「暂无日志」

**工期**：~1.5 工日

### Batch G：资源生命周期（2 项）—— Chunk 7
**SSE / 子进程 与 API/UI 退出钩子脱钩**

- **G1** `api_stop` / `api_rotate_token` 只 join accept 线程，SSE writer 线程不被追踪 → 旧 hub `tx` 永不 drop → 旧客户端以为活着永收不到新事件
- **G2** `run_python` 读阶段无整体兜底超时（与 C1 同根，**修了 C1 自动覆盖**）

**工期**：~1.5 工日（含 C1 工作量）

### Batch H：跨平台打包配置（2 项）—— Chunk 8
**P0-8A/B 修完后剩**

- **H1** 无 `bundle.resources`、无 `bundle.windows` 节 → `WebView2Loader.dll` 未声明为资源（老板 2026-08-14 实测坑）
- **H2** 无 `bundle.macOS` 节（signingIdentity / hardenedRuntime / entitlements）→ 本地 dev 正常，release `.dmg` 是 ad-hoc 签名，Gatekeeper 拒收

**工期**：~1.5 工日

---

## 4. P2 长期清单（35 项）

按文件归档（详见 `AUDIT-REPORT-KIMI-2026-08-18.md` §4）：
- api_handlers / api_auth：3 项（P2-1 token 时序 / P2-2 日志注入 / P2-3 SSE replay race）
- db / migration：5 项（P2-4 WAL 丢失 / P2-5 多语句无事务 / P2-6 TOCTOU / P2-7 误迁移 / P2-8 规则覆盖）
- bot_py：4 项（P2-9 临时目录泄漏 / P2-10 探测无缓存 / P2-11 审计注入 / P2-12 并发无闸门）
- middleware / audit：4 项（P2-13 无 catch_unwind / P2-14 双 Vec 分离 / P2-15 写失败静默 / P2-16 kv 不转义）
- profile：3 项（P2-17 头像读无上限 / P2-18 删旧写新 / P2-19 目录解析漂移）
- 前端：4 项（P2-20 全量刷新 / P2-21 mutate 直改 / P2-22 回收站无序 / P2-23 双实现漂移）
- bot_py / lib：3 项（P2-24 ExitRequested 无清理 / P2-25 spawn 失败漏目录 / P2-26 sleep 冻结 UI）
- error / bot_chat / 跨平台：3 项（P2-27 错误分层缺失 / P2-28 业务拒绝降级 / P2-29 版本号 4 处漂移）
- capabilities / Cargo：3 项（P2-30 opener 通配 / P2-31 Linux 托盘缺失 / P2-32 keyring Linux 探测）
- WidgetApp / IPC：3 项（P2-33 轮询静默 / P2-34 catch 静默 25 处 / P2-35 recoverable 未消费）

**工期**：~5 工日（合并式修复可省 1-2 工日）

---

## 5. 跨主题横切（建议放 P1 修复时同步做）

1. **统一原子写**：`fs::write` → `atomic_write(path, contents)` helper（profile.rs / save_rules / db.rs）
2. **统一数据目录解析**：profile.rs:55 vs db.rs:50 → 单一 `data_dir()`
3. **统一审计转义**：`truncate_for_log` → `escape_for_log`（剥 `\n` / `| `）
4. **统一 Mutex 模式**：profile + DB write 进程内 Mutex

---

## 6. 排期建议（更新）

| 周次 | 任务 | 工日 |
|---|---|---|
| **本周内（立即）** | P0 全清（6 项，3 批） | 1.5 |
| **下周** | Batch A（API 5）+ Batch B（DB 4） | 7.5 |
| **第三周** | Batch C（Python 4）+ Batch D（中间件/profile 4） | 5.5 |
| **第四周** | Batch E/F/G/H（前端+错误+资源+打包 12 项） | 6 |
| **长期** | P2 35 项合并式修复 | 5 |
| **总额** | —— | **~25.5 工日** |
| **真实可压** | 横切 + P2 合并 | **~18-20 工日 ≈ 4 周** |

---

## 7. 决策项（待老板定）

- [x] P0-1 已修（2026-08-18 18:54）
- [ ] **P0-α / P0-β / P0-γ 是否本周内全清？**（建议：必清，1.5 工日 release 必过）
- [ ] Batch A（API）3.5 工日，本周内清还是下周？
- [ ] 横切关注点（§5）是否一并做？
- [ ] P2-4（WAL 丢失）和 P2-15（审计静默）建议升 P1 处理
- [ ] pre-existing 2 个 test fail（C 路径 DSL migration 遗留）何时单独立项修

---

## 8. 修复执行策略

**对 Kimi code 的指令模板**（每次 1 个 batch）：
- 工作目录：`/Users/renshi/Projects/wmessage`
- 指定文件:行号 + 症状 + 修复策略（具体到"改哪一行"层面）
- 必须跑 verify：`cd src-tauri && /Users/renshi/.cargo/bin/cargo build` + `cargo test --lib` + 前端 `pnpm tsc --noEmit`
- 严禁：drift 到其他文件 / 其他 bug；改 API；改业务逻辑超出指定范围
- 完成 → git commit + 简明 commit message

**串行策略**：1 次 Kimi 调用 = 1 个 batch，**严格串行**，禁止并行 / sub-agent / Explore / Task（2026-08-18 6 并行打爆 403 教训）