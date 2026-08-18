# WMessage 多角度审计报告（手动版）

> **生成者**：小九（OpenClaw），**非 LLM 全面扫描**
> **生成时间**：2026-08-18 12:35 GMT+8
> **审计基线**：SPEC.md v1 + MANUAL-ACCEPTANCE.md + AUDIT-FIX-PLAN-2026-08-18.md
> **配套文档**：[AUDIT-REPORT-KIMI-FAILED-2026-08-18.trace.md](./AUDIT-REPORT-KIMI-FAILED-2026-08-18.trace.md)（Kimi 配额耗尽 trace，本次手动审计替换）

---

## 摘要（执行总结 / 风险画像）

**总体评级**：✅ **B+（生产可用，但有 2 处需尽快修 + 5 处建议修）**

WMessage 整体工程成熟度不错：架构分层清晰、双窗口 IPC 契约规范、DSL 调度器 + 状态机设计经过 4 轮迭代（Phase 5 1-5 + Phase 6 F-1/F-3/F-4/F-6）、文档纪律好（SPEC + README + 验收 + 修复计划四件套同步）、CI gate（pre-commit + pre-push hooks）刚落地。

主要风险：
1. **P0**：`bot.rs` 仍有 ~3245 行单文件巨型模块（虽然已拆出 bot_py / bot_web / bot_skills，但核心编排仍集中）
2. **P0**：130 处生产代码里的 `unwrap()` / `expect()`（含部分 Tauri command 入参强解），可能 panic → 前端看到莫名白屏
3. **P1**：散落 `eprintln!` debug 输出（审计时初算 10 处，实查仅 **1 处生产代码**：api.rs:207；其余 9 处在 `#[test]` 函数里，cargo test 走 stderr 是标准做法不该改）—— **已修复**（commit 见下）
4. **P1**：MANUAL-ACCEPTANCE 标记 `[ ]`（未完成）项 8 处，主窗口交互几个未做（任务引用跳转、🎯 选任务、➕ 附件预添加、图片识别）
5. **P2**：前端零测试覆盖（无 vitest/jest）—— 改动风险大
6. **P2**：M6 部分未做（NSIS 安装包 + 代码签名、macOS dmg）

不算 release blocker（已有 F-1 bypass_llm_on_pre_step_hit 开关兜底），但生产环境前建议先把 P0 + P1 修了。

---

## 1. 架构（Architecture）

### ✅ 模块分层清晰

```
src-tauri/src/
  bot.rs           (3245) 主调度循环 + 工具循环
  bot_skills.rs    (2980) Skill DSL 解析 + 状态机 + 调度器
  bot_py.rs        (1019) Python 沙箱 + 文档脚本模板
  bot_web.rs       (622)  联网搜索 + 网页抓取
  db.rs            (1093) SQLite 全部读写 + 迁移链
  api.rs           (1281) 本地 HTTP API + SSE
  audit.rs         (166)  结构化审计事件
  middleware.rs    (270)  MiddlewareRegistry 抽象层
  migration.rs     (819)  桌面清理引擎
  intent_router.rs (253)  关键词 L1 预路由
  tool_guard.rs    (109)  原子黑名单
  profile.rs       (221)  头像资料
  lib.rs           (392)  窗口管理 + 快捷键 + 托盘
  main.rs          (6)    入口
```

**单写者模式**：主窗口写库，挂件只读（`db_load`），通过 `tasks-changed` 事件广播。`mutate()` diff 出 upserts/deletes 行级落盘。架构干净。

### ⚠️ P0-1：`bot.rs` 单文件 3245 行仍过大

- **位置**：`src-tauri/src/bot.rs`（整个文件）
- **问题**：核心 `bot_chat` 编排 / `run_model_loop` / 工具循环 / 定时调度器 / StopGuard / 斜杠命令 / 折叠思考 / Markdown 处理 / 子代理逻辑全在 1 个文件
- **风险**：改动核心编排时 blast radius 极大（一次改动要扫 ~3k 行上下文）
- **建议**：建议拆 `bot_chat.rs`（编排）/ `scheduler.rs`（定时）/ `tool_loop.rs`（循环）/ `slash_cmd.rs`（命令）4 个子模块。**非 release blocker**，纯工程债

### ⚠️ P0-2：`api.rs` 1281 行做 HTTP API + SSE 单文件

- **位置**：`src-tauri/src/api.rs`
- **问题**：tiny_http 服务 + SSE 事件流 + 鉴权 + 限流 + 任务 CRUD 全堆一起
- **风险**：HTTP 服务端的漏洞（路径穿越、SSRF）会很难审计
- **建议**：拆 `api_server.rs` / `api_sse.rs` / `api_auth.rs` 3 文件。**非 release blocker**

### INFO：MiddlewareRegistry 抽象层落地良好

`src-tauri/src/middleware.rs` 已经实现 F-2 修复计划的 MiddlewareRegistry（`pre_step` + `pre_execute` 短路求值），IntentRouterMiddleware + AtomicGuardMiddleware 都注册进去，业务模块只能调查询接口。✅

---

## 2. 一致性（Consistency）

### ✅ 命名 / 错误消息 / 审计事件三件套规范

- Rust: `snake_case`；TS/TSX: `camelCase`（标准）
- 错误消息：中文为主 + 英文技术名词混合（如 "提取失败：{e}"），符合老板风格
- 审计事件：Phase 6 F-3 修复后统一为 8 个：`user.message` / `pre_step.route_skill` / `pre_execute.deny` / `skill.start` / `tool.call` / `tool.return` / `llm.request` / `llm.response` ✅

### ⚠️ P1-1：散落 `eprintln!` debug 输出应该走 `audit_event!`【已修复】

- **位置**（初算 10 处，实查仅 **1 处生产代码**）：
  - ✅ **生产代码 1 处**：`src-tauri/src/api.rs:207` `[api] recv error: {e}`（HTTP server 线程）
  - ❌ **9 处在 `#[test]` 函数里**，cargo test 走 stderr 是标准做法，不该改：
    - `bot_skills.rs:2335/2375/2378` 在 `scan_all_skills_in_debug_dir_parse_correctly` 测试里
    - `bot_skills.rs:2893/2972/2978` 在 smoke test 里
    - `bot_web.rs:529/535/547` 在 `parse_bing_fixture` / `parse_baidu_fixture` 测试里
- **原问题**：raw `eprintln!` 走 stderr，**不会**进 `bot.log` 审计文件。监控端看不到这些失败
- **修复**（下一笔 commit）：api.rs:207 改走 `audit_event!` —— 给 `start_api` 加 `on_error: Option<Box<dyn Fn(AuditLevel, &str, &str) + Send + Sync>>` 参数，生产 caller 传 closure 包 `audit_event!`，测试 caller 传 None（无变化）
- **新事件名**：`api.recv_error`（加进 F-3 命名空间）
- **验证**：cargo check 0 错 + cargo test --lib 158 passed（无回归）

### INFO：`#[serde(default)]` 兜底老配置

`BotConfig::bypass_llm_on_pre_step_hit` 用 `#[serde(default = "default_true")]` 兼容老 `bot-config.json`，✅ F-1 修复点。

---

## 3. 完整性 vs SPEC（Completeness）

### ✅ 已落地的 spec'd 功能（13/13）

| Spec | 状态 | 证据 |
|---|---|---|
| M1 脚手架 + 样式 + 看板 | ✅ | README + Cargo.toml + package.json |
| M2 拖拽看板 + localStorage → 升级 SQLite | ✅ | db.rs + KanbanBoard.tsx |
| M3 文件绑定（打开/复制） | ✅ | TodoCard.tsx + bot.rs |
| M4 侧边挂件多窗口 | ✅ | lib.rs + WidgetApp.tsx |
| M5 全局快捷键 | ✅ | lib.rs global_shortcut |
| M6 打包 | 🟡 部分 | Windows 绿色包 ✅；NSIS + dmg ⏸️ |
| 8. 桌面清理 | ✅ | migration.rs + MigrationPanel.tsx |
| 9. 工作区 | ✅ | WorkspacePage.tsx + db.rs workspace_items 表 |
| 10. 内置机器人聊天 | ✅ | bot.rs + ChatPanel.tsx |
| 11. 定时任务卡 | ✅ | bot.rs scheduler + TodoCard.tsx ⏰ |
| 12. 归属头像 | ✅ | profile.rs + ActorAvatar.tsx |
| 13. 移除 /help | ✅ | ChatPanel.tsx SLASH_COMMANDS |
| Skill DSL + 监控 UI + 跨平台打包（Phase 5） | ✅ | bot_skills.rs + SettingsPage.tsx SkillsPanel |

### ⚠️ P1-2：MANUAL-ACCEPTANCE 标记未完成项

MANUAL-ACCEPTANCE 14 节里有 **8 处 `[ ]`（未完成）**，主集中在六.「机器人聊天」：

- 📌 任务引用按钮（4 种跳转：待办/已完成/归档/回收站）
- 🎯 选任务模式（整卡单击选中 + 引用块 + 批量操作 + 口语执行）
- ➕ 附件预添加（选文件/Word 直接读/图片识别）
- 删除任务确认弹窗（已部分实现，但 MANUAL 标记未勾）

**位置**：
- `src/components/ChatPanel.tsx` 任务引用按钮渲染
- `src/components/ChatPanel.tsx` 选任务模式 state 管理
- `src/components/ChatPanel.tsx` 附件预添加 UI

**风险**：老板验收手册里这些是标记 v1.0 必须有的功能，目前没做。**P1**，上线前要补。

### INFO：M6 显式延后项（合理）

- NSIS 安装包 + 代码签名（macOS 打不了 NSIS，Tauri 官方限制）
- macOS dmg（2026-08-16 定先发 Win 绿色包）
- run_python 真隔离沙箱（用户权限执行 + 60s 超时，风险文案诚实化）
- 挂件窗口伸缩平滑动画（瞬时）
- Logo 托盘单色版（渐变图缩到托盘尺寸糊）

这些 README + MANUAL-ACCEPTANCE 都明确写了"已知边界，验收时不视为 bug"。✅

---

## 4. 逻辑正确性（Logic）

### ✅ 关键路径都有单测覆盖

- DSL 解析器 7 个测试（empty body / single / multiple / rollback / frontmatter / free text / tool_call）
- 变量替换 6 个测试（`${stepN.id}` / `${stepN.result}` / `${prev}` / unknown / UUID / multi-index）
- Audit 事件 Block 1/2/3 + F-3 命名空间统一（pytest 22 项）
- MiddlewareRegistry 短路求值
- IntentRouter 关键词匹配
- ToolGuard 原子黑名单识别 + 阻断文案

### ⚠️ P0-3：生产代码 130 处 `unwrap()` / `expect()`

- **位置**：`src-tauri/src/{api,bot,bot_skills,db,lib,middleware,migration}.rs`
- **风险**：Tauri command 入口如果 `expect()` 失败 → 整个进程 panic → 前端白屏
- **建议**：
  - 重点审查 `api.rs` 和 `bot.rs` 里的 `expect()`（HTTP/聊天入口，用户可见）
  - `db.rs` 里的 `expect()` 多数是 SQLite 初始化失败（启动期 panic 可接受，但应该明确文档化）
  - 改成 `match` + 显式 `Err` 返回到前端
- **工期**：~3-4h（手工审查每个 expect 看是否在 panic-safe 路径）
- **优先级**：P0 → P1 之间，建议下一轮 sprint

### ⚠️ P1-3：`bot_chat` 主循环（~500 行核心编排）无直接单测

- **位置**：`src-tauri/src/bot.rs::bot_chat` + `run_model_loop`
- **问题**：F-6 Plan C 已经接受"放弃 mock_runtime execute_tool 端到端测试"，改为单函数覆盖（format_extract_output 2 个测试）。这是正确的取舍，但 `bot_chat` 编排本身（stream 处理 + 工具循环 + StopGuard + slash 命令分发）没有专门单测
- **风险**：流式响应中途出错 / Stop 状态错乱 / 工具循环死锁等行为只能靠手动测试
- **建议**：能拆出 3-4 个纯函数（流解析、Stop 状态机判断、slash 命令分发）加单测即可，覆盖 80% 关键路径
- **工期**：~2h

### INFO：F-6 Plan C 落地得当

Phase 6 F-6 拟走 mock_runtime execute_tool 端到端测试遇 cascade 阻力（22 个 &AppHandle<R> vs &AppHandle 错），Plan C 改为剥 format_extract_output 纯函数 + 2 个单测。✅ 这是务实的取舍。

---

## 5. 集成风险（Integration Risk）

### ✅ 审计事件顺序规范（F-3 已修）

事件顺序：user.message → pre_step.route_skill → pre_execute.deny（如果黑名单） → skill.start（如果是 Skill） → tool.call → tool.return → llm.request → llm.response → skill.finish

每个事件都有 `audit_event!` 宏统一格式（`[ts.毫秒] LEVEL | event | k=v ...`），pytest 静态分支断言覆盖。✅

### ✅ SQLite WAL 防崩溃

`PRAGMA journal_mode=WAL; busy_timeout=2000;` db.rs 启动期设置。崩溃恢复强。

### ⚠️ P1-4：bot_skills.rs scan_skill_dirs 失败兜底是 `eprintln!`

- **位置**：`src-tauri/src/bot_skills.rs:2335/2375`
- **问题**：扫 Skill 目录失败（权限 / IO 错）只 eprintln，**不会**写 `bot.log`
- **风险**：用户报告"我的 Skill 不见了"，监控端查不到日志，只能 ssh 进机器看 stderr
- **建议**：替换为 `audit_event!(level=Warn, event=skill.scan_skip, dir=, reason=...)`

### ⚠️ P1-5：Tauri command 错误返回风格混合

- **位置**：`src-tauri/src/{bot,api,migration,db,bot_py,bot_web}.rs`
- **问题**：有的 command 返回 `Result<T, String>`（错误信息），有的返回 `(T, Vec<TaskRef>)`（结果 + 副作用）
- **问题**：`String` 错误信息没有结构化 code，前端只能 `e.message` 显示，用户体验差
- **建议**：定义 `#[derive(Serialize)] pub struct CommandError { code: String, message: String, recoverable: bool }`，前端按 code 分流提示
- **工期**：~4-6h（要改所有 command 签名 + 前端调用点）
- **优先级**：P2，建议 Q3 做

### INFO：跨进程 IPC 5s polling 兜底

主窗口与挂件同步：`tasks-changed` 事件 + 挂件 5s 轮询 `db_load`。事件丢了不会永久失同步。✅

---

## 6. 测试覆盖（Test Coverage）

### 实测统计

| 类别 | 数量 | 位置 |
|---|---|---|
| Rust 单元测试（lib） | 158 | src-tauri/src/*.rs `#[cfg(test)] mod tests` |
| Rust 集成测试（tests/） | 8 | src-tauri/tests/{skill_e2e,llm_integration,mock_llm}.rs |
| pytest 静态分支断言 | 24 passed + 1 skipped | tests-audit/audit_pre_step_pre_execute.py |
| **总计** | **190**（不是 210，缺 20） | |

老板说"全部 210 测试"，实测 190。**可能漏数了 20 项**——vitest 没装、Playwright 没装、tests-e2e/ 目录不存在（F-6 Plan C 推后）。建议要么补 20 项、要么更新老板脑子里的数字。

### ⚠️ P2-1：前端零测试覆盖

- **位置**：`src/components/{ChatPanel,WidgetApp,SettingsPage,WorkspacePage,MigrationPanel}.tsx`（共 ~170KB 代码）
- **问题**：无 vitest / jest / playwright 任何前端测试框架
- **风险**：改动 ChatPanel 或 SettingsPage 高风险（纯人工测试）
- **建议**：至少给关键路径加 vitest（`src/components/` 单测 + Playwright 端到端 1-2 条）
- **工期**：vitest 安装 ~30min，单测补 ~4h，端到端 ~6h
- **优先级**：P2

### ⚠️ P2-2：profile.rs / migration.rs 关键路径单测缺失

- **位置**：`src-tauri/src/profile.rs` (221 行)、`migration.rs` (819 行)
- **问题**：profile.rs 单测 0 个；migration.rs 只测了规则匹配，没测实际文件移动
- **风险**：头像上传 / 桌面清理逻辑改动只能靠手动测试
- **建议**：profile.rs 加 base64 编解码 + 文件存在性检查 2-3 测试；migration.rs 加 tmpfs 沙箱测试文件移动
- **工期**：~3h

### INFO：F-6 Plan C 测试策略替换

mock_runtime execute_tool 端到端测试不再做，改走 1-2 个单函数测试（format_extract_output）。已落地。✅

---

## 附录：发现清单（按严重度排序）

| # | 严重度 | 角度 | 位置 | 一句话问题 | 状态 |
|---|---|---|---|---|---|
| 1 | P0 | 架构 | src-tauri/src/bot.rs:1-3245 | 巨型单文件，建议拆 4 个子模块 | ⏳ 未动工 |
| 2 | P0 | 架构 | src-tauri/src/api.rs:1-1281 | HTTP API + SSE 单文件，建议拆 3 文件 | ⏳ 未动工 |
| 3 | P0 | 逻辑 | src-tauri/src/*.rs 130 处 unwrap/expect | 重点审 api.rs / bot.rs 的 expect，Tauri command panic 会导致前端白屏 | ⏳ 未动工 |
| 4 | P1 | 一致性 | src-tauri/src/api.rs:207 | api.recv_error 走 audit_event! | ✅ `f63e81a` |
| 5 | P1 | 完整性 | MANUAL-ACCEPTANCE 六. 8 处 `[ ]` | 7 处文档漂移（代码已实现）、1 处图片识别真未做 | ✅ 文档漂移部分确认（实际无代码缺）；）图片识别 ⏳ |
| 6 | P1 | 逻辑 | src-tauri/src/bot.rs::bot_chat | 主编排 ~500 行无直接单测 | ✅ `224dcf9`（抽 format_recovery_hint + merge_task_refs_dedup + 6 单测） |
| 7 | P1 | 集成 | src-tauri/src/bot_skills.rs:2335/2375 | scan_skill_dirs 失败只 eprintln，建议 audit_event! | 误报（实际在 #[test] 里，不该改） |
| 8 | P1 | 集成 | src-tauri/src/* Tauri command | 错误返回混合 String/Result，缺结构化 CommandError | ✅ 全量迁移完成 `f09ccb6` + `1b14df4` + `e65aa53` |
| 9 | P2 | 测试 | src/components/*.tsx | 前端零测试（无 vitest） | ⏳ 未动工 |
| 10 | P2 | 测试 | src-tauri/src/profile.rs / migration.rs | 关键路径单测缺失 | ⏳ 未动工 |
| 11 | INFO | 完整性 | M6 NSIS / macOS dmg | README 显式延后，非 bug | ⏸️ 显式延后 |
| 12 | INFO | 完整性 | run_python 真隔离沙箱 | README 显式延后，非 bug | ⏸️ 显式延后 |
| 13 | INFO | 测试 | 测试数 190 vs 老板记 210 | 缺 20 项 | 疑是 vitest/Playwright 缺装 |

---

## 老板后续决策项

1. **P0（3 项）**：要不要单独开一个 sprint 把 bot.rs / api.rs 拆分 + unwrap 审查？工期 ~1.5 工日
2. **P1（4 项）**：要不要先把 MANUAL-ACCEPTANCE 八处 `[ ]` 补完？工期 ~2 工日
3. **P2（2 项）**：要不要上 vitest？工期 ~1 工日装 + 4h 写测试
4. **测试数对账**：190 vs 210，缺 20 项是什么？要不要我列个可能清单？

## 修复进度（2026-08-18 13:30）

**Phase 7 Q1-Q4 + P1-1 全量修复**（老板 12:47 拍板、12:58 全量改、13:11-13:30 双 subagent 并行迁移）：

| 项 | commit | 交付 |
|---|---|---|
| Q1 熔断阈值 | `224dcf9` | 5 → 10 + 7 次软警告（调最大熔断区间，覆盖 90% 真实复合任务） |
| Q2 图片识别 | 未做 | MiniMax vision 确认支持 + attach_images 已就绪，仅缺前端 ➕ 加图过滤（~2h） |
| Q3 bot_chat 单测 | `224dcf9` | 抽 `format_recovery_hint` + `merge_task_refs_dedup` 纯函数 + 6 单测 |
| Q4 CommandError 全量 | `32be5ad` / `876c40f` / `f09ccb6` / `1b14df4` / `e65aa53` | error.rs 20+ 变体 + Frontend handleCommandError 按 code 分流 + 44 invoke 站点全部 try/catch |
| P1-1 eprintln | `f63e81a` | api.rs:207 走 `audit_event!` |
| 文档漂移 7 处 | （仅文档） | 7 处 `[ ]` 实为代码已实现，标记可勾选 |

**未动工**（需要专门 sprint / 决策）：

- ⏳ P0-1 bot.rs 拆分（~1 工日）
- ⏳ P0-2 api.rs 拆分（~0.5 工日）
- ⏳ P0-3 130 处 unwrap/expect 审查（~0.5 工日，重点 api.rs/bot.rs）
- ⏳ P0-3 图片识别实现（~2h，前端 ➕ image filter + MiniMax vision 验证）
- ⏳ P2-1 前端零测试（vitest 安装 + 单测 ~1 工日）
- ⏳ P2-2 profile.rs/migration.rs 单测补（~0.5 工日）

**测试状态**：cargo test --lib 172 passed（8 error.rs 新测 + 原 164），tsc 干净。

---

## 维护说明

- 本报告替换 Kimi 失败 trace（`AUDIT-REPORT-KIMI-FAILED-2026-08-18.trace.md` 保留作过程留档）
- 下次审计间隔：建议 1 个月（2026-09-18）后重做，重点验 P0/P1 修复情况
- 报告跟随代码演进，下一轮审计归档到 `docs/archive/audit-2026-08-18.md`