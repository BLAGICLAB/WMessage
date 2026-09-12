# WMessage Multi-Skill Audit Report (2026-09-12)

> **视角**: Scissors-Slice (大文件切片) + ponytail-audit (全仓过度工程) + ponytail-review (具体代码评审)
> **目标**: 项目根 `/Users/renshi/Projects/wmessage`
> **约束**: 只产出报告，未改任何代码 / 结构 / 依赖
> **基线**: 99 个代码文件 / ~50K 行 (`wc -l` 累计) / Cargo.lock 含 685 crates

## 0. TL;DR（执行摘要）

| 维度 | 关键发现 | 优先级 |
| --- | --- | --- |
| **超大文件** | 7 个 Rust 文件 >1800 行，承担 ≥3 类独立职责，是切片第一梯队 | 🔴 P0 |
| **Scissors-Slice 候选** | `bot_py.rs`(3677)、`bot.rs`(3444)、`db.rs`(2693)、`migration.rs`(2283)、`api_handlers.rs`(1924)、`bot_chat.rs`(1875)、`bot_model_loop.rs`(1852)、`SettingsPage.tsx`(1846)、`ChatPanel.tsx`(1493) | 🔴 P0 |
| **ponytail-audit 候选** | bot.rs 三套 `secret_service_available` cfg 重复；`bot_skills/state.rs` `test_hook_*` 与 `test_*` 双套测试入口；多处 `Option<u32>`→默认值的薄包装函数 | 🟡 P1 |
| **ponytail-review** | `bot_web.rs` 手写 HTML 解析（已引 `html2text`），`api_handlers.rs` 自研限流（无 crate 替代），`db.rs` `#[tauri::command]` 散落文件底部（违反 cmd→model→db 分层） | 🟡 P1 |
| **架构腐化点** | `lib.rs` `mod` 列表与 `Cargo.toml` 依赖体量都在扩张；`tests-audit/` 两个 pytest 已能拦 P0 级问题，但 run 不过历史回归基线 (`>=400`) | 🟢 P2 |

> 历史审计沉淀（`docs/AUDIT-*` 共 21 份，2026-08-18 ~ 2026-09-08）已覆盖：API、BOT、DATA、ERROR、SCHED、SECURITY、PLATFORM、CONTRACT、TESTS、SUMMARY、LLM。阶段5已固化为 `tests-audit/audit_tauri_bridge.py` + `audit_pre_step_pre_execute.py`（仅静态分支断言，不跑业务代码，避免污染 spec 测试）。本报告**不复述历史结论**，聚焦**未审计**或**审计后未切片**的大文件责任层。

---

## 1. Scissors-Slice 角度 — 大文件责任切分分析

> Scissors-Slice v1.1.0 原则：识别 mixed concerns，按 domain 聚合，**不实际拆**。本节只标责任边界，给出"如果将来要拆"的建议方向。

### 1.1 `src-tauri/src/bot_py.rs` (3677 行) — **最大单文件**

按行号识别出 **5 个独立职责** 交织在同一文件：

| 行号段 | 职责 | 抽出后可命名 | 复用度 |
| --- | --- | --- | --- |
| L1–250 | Python/DotNet 解释器探测 + 缓存 | `python_env.rs` | 一次性内部 |
| L287–560 | `PyEnv` / `RunLimits` / 子进程限额 (rlimit) | (留原文件) | 高 |
| L565–790 | 子进程注册表 + Drop 守卫 (`ChildRegGuard`) + 退出时清理 | `py_children.rs` | 中 |
| L804–1247 | `run_python` / `run_python_ungated` / `run_python_at` 核心调度 + output 收集 + harvest | `python_runtime.rs` | 高 |
| L1191–1247 | `sweep_stale_py_runs` 后台清理 | (可并入 runtime) | 低 |
| L1328–2205 | 审计日志 + `py_exec_sync` + `py_exec_sync_async` | `py_audit.rs` | 低 |
| L2205–3677 (尾部) | **6 个 `doc_*` 命令**（doc_extract / doc_make_word / doc_make_word_revisions / doc_make_excel / doc_make_pdf / doc_make_ppt）+ **嵌入的 Python 脚本字面量**（EXTRACT_SCRIPT、MAKE_DOCX_SCRIPT、MAKE_DOCX_REVISIONS_SCRIPT、MAKE_XLSX_SCRIPT、MAKE_PDF_SCRIPT、MAKE_PPT_SCRIPT — 合计占大量行） | `doc_commands.rs` + `doc_scripts.rs` | 高（产物落 AI_Gen_Files） |

**Scissors-Slice 标记**:
- `mix-1` Python 进程生命周期（探测/缓存/限额/注册表/收割）与文档生成 API **没有任何共享状态**，却被塞在同一文件。
- `mix-2` Python 脚本字面量（接近文件体积 1/3 是字符串）与 Rust 调用层混在一起，导致 `cargo fmt` / `clippy` 频繁误碰字符串内容。
- **建议切片顺序**: `doc_scripts.rs` 优先（最大单收益：纯字符串，无依赖）；`python_runtime.rs` 次之；其余按需。

### 1.2 `src-tauri/src/bot.rs` (3444 行) — 配置 + 凭据 + 旧迁移

| 行号段 | 职责 | 抽出后可命名 |
| --- | --- | --- |
| L64–360 | `KeySlot` / `BotConfig` / `ModelsByProvider` / `ActiveModelId` + 旧迁移 (`migrate_legacy_models`, `derive_legacy_fields_from_active`, `derive_default_label`) | `bot_config.rs` |
| L367–530 | `PermMode` / `key_backend` / `secret_service_available`(3 个 cfg 变体) / `linux_app_data_dir` (2 个变体) / `warn_fallback_once` | `bot_keys.rs` |
| L552–770 | 凭据读写 (`read_api_key_at`, `has_api_key_at`, `write_api_key_at`, `delete_api_key_at`, `read_key_of_slot`, `write_key_of_slot`, `migrate_plaintext_key_if_system`) | `bot_key_storage.rs` |
| L775–1073 | `BotConfigView` + `migrate_legacy_key` + `migrate_search_keys` + `migrate_search_key_slot` + `load_config` / `add_allowed_dir` | `bot_config_view.rs` |
| L1073–3444 (尾) | 14+ 个 `#[tauri::command]` 集中区 | `bot_commands.rs` |

**Scissors-Slice 标记**:
- `mix-1` BotConfigView 跟 BotConfig 同一文件，但其本质是"前端透传 DTO"，可以独立。
- `mix-2` 三个 cfg 变体的 `secret_service_available` + 两个 cfg 变体的 `linux_app_data_dir` 是平台分支样板，**有可能属于 stdlib 范畴**（详见 §3）。
- `mix-3` `KeySlot` enum 后每加一个槽位就要复制 6 个 `read_/has_/write_` 函数 — 抽象过度展开信号。
- **建议切片顺序**: `bot_config.rs` 优先（最大耦合出口：其他模块都 `use crate::bot::*`）。

### 1.3 `src-tauri/src/db.rs` (2693 行) — 数据层 + 命令绑定

| 行号段 | 职责 | 抽出后可命名 |
| --- | --- | --- |
| L13–217 | `Subtask` / `Task` / `TaskFile` / `WorkspaceLink` / `BotMsgRow` / `BotSession` / `WorkspaceItem` / `PersistedSkillOutcome` + 路径 helper | `db_types.rs` |
| L217–460 | `open_db` + WAL sidecar + 旧 DB 拷贝 + 文件列 schema 演进 + 文件绑定迁移 | `db_open.rs` |
| L496–689 | `reset_bot_assigned_with` + workspace load/upsert/delete | `db_workspace.rs` |
| L703–927 | `bot_sessions_load/create/rename/delete` + `bot_history_save_inner` | `db_bot_sessions.rs` |
| L981–1209 | `upsert_tasks` / `delete_tasks` / `load_all` | `db_tasks.rs` |
| L1209–1487 | 旧 JSON 迁移 + 导入/导出 | `db_migrate.rs` |
| L1487–2693 (尾) | 14+ 个 `#[tauri::command]` 集中在文件底部 | `db_commands.rs` |

**Scissors-Slice 标记**:
- `mix-1` **DB 模型 / DB 核心 / DB 命令三层混在同一文件**，违反 `model → use_case → tauri::command` 标准分层。最严重处：L703 起 6 个会话命令 + L927 起 1 个 history 命令 + L1487 起 2 个导入导出命令，全部 `pub fn` 直接绑 `AppHandle`，无法 mock 单测。
- `mix-2` `load_all` 长度 113 行（L1096–1209），内含字段映射逻辑；应拆 `Task::from_row` + 单独 `load_all`。
- **建议切片顺序**: `db_commands.rs` 优先（解耦最大；现有 `tests-audit/audit_tauri_bridge.py` 已能兜底），`db_tasks.rs` 次之。

### 1.4 `src-tauri/src/migration.rs` (2283 行) — 规则引擎 + journal + 轮询

| 行号段 | 职责 | 抽出后可命名 |
| --- | --- | --- |
| L41–211 | `JournalEntry` + `journal_pending/committed/cleared/find_pending` + 对账策略 `decide_src_missing` | `migration_journal.rs` |
| L339–408 | `recover_move_db` + `recover_delete_db` + `MigrationGuard` | (可并入 journal) |
| L411–597 | `MigrationRule` + `RulesFile` + `load_rules` / `save_rules` / `validate_rules` / `parse_rules_csv` + `template_csv` | `migration_rules.rs` |
| L614–706 | 文件操作 (`copy_dir_recursive` / `move_entry` / `claim_dst_name` / `conflict_free_name` / `move_remove_permanently_failed` 计数器 / `log_line` / `emit_upserts`) | `migration_fs.rs` |
| L715–1088 | `run_migration` / `run_migration_inner` **— 这是文件最大单函数** | `migration_runner.rs` |
| L1088–1112 | `spawn_polling` 后台轮询 | (可并入 runner) |
| L1121–1416 | 6 个 `#[tauri::command]` | `migration_commands.rs` |

**Scissors-Slice 标记**:
- `mix-1` **`run_migration_inner` 单函数 L720–1088 ≈ 370 行**，嵌套 `match rule.action` 多次复制"读旧任务→新建 nt→设 updated_at→落盘→commit journal"模板。**ponytail-review §3 给具体行号与 shrink 候选**。
- `mix-2` `expected_updated_at = t.updated_at` 模板在文件内**重复 ≥5 次**（NEW-B-1 / T1-1 注释提到）— 应抽 `mark_loaded(&mut nt)` / `prepare_rmw_baseline(&mut nt, prev: &Task)`。
- `mix-3` journal 模块对外只暴露 5 个函数，但内部还有 `_inner` + lock 取锁，是典型的 facade 反复 copy。
- **建议切片顺序**: `migration_runner.rs` 优先（最大单收益：可单测）；`migration_journal.rs` 次之。

### 1.5 `src-tauri/src/api_handlers.rs` (1924 行) — HTTP API + SSE + 限流

| 行号段 | 职责 | 抽出可命名 |
| --- | --- | --- |
| L65–343 | `ApiInfo` / `ApiStatus` / `handle_request` 路由分发 + JSON 工具 + 健康检查 + 限流 + 日志 + body 读取 + 时间戳 | `api_dispatch.rs` |
| L345–825 | CRUD：list / get / create / update / delete | `api_crud.rs` |
| L825–997 | SSE 注册/停止 + `sse_connect` | `api_sse.rs` |
| L998–1175 | `api_start/stop/status/rotate_token` 4 个 `#[tauri::command]` | `api_admin.rs` |

**Scissors-Slice 标记**:
- `mix-1` `update_task` 单函数 L586–765 ≈ 180 行：参数校验 + 字段合并 + 列派生 + 日志 — 可拆 `update_task_validate` + `update_task_apply`。
- `mix-2` 限流 `rate_check`（L180–193）单文件内闭包实现，**不属于 stdlib，但属于 hand-rolled infra**（详见 §3）。

### 1.6 `src-tauri/src/bot_chat.rs` (1875 行) — 聊天编排 + 任务执行

| 行号段 | 职责 | 抽出可命名 |
| --- | --- | --- |
| L35–460 | `ChatMsg` + 摘要/截断/历史合并 + memory block + 图片附件 + think block 剥离 | `chat_history.rs` |
| L458–545 | `TaskRef` + `BotChatResult` + `ChatGuard` 防重入 | `chat_types.rs` |
| L546–897 | `bot_chat` / `bot_compact` 等 `#[tauri::command]` | `chat_commands.rs` |
| L1093–1432 | `ExecGuard` + `TaskExecOrigin` + `create_exec_session` + `persist_exec_reply` | `chat_executor.rs` |
| L1432–1875 (尾) | `build_task_block` + 其它执行辅助 | (留原文件) |

**Scissors-Slice 标记**:
- `mix-1` 防重入有两种守卫：`ChatGuard`(L505) 与 `ExecGuard`(L1100) — 都是 `Mutex<HashSet>` + Drop RAII，**抽象完全可以复用**（详见 §3）。
- `mix-2` `TaskExecOrigin::as_str` + `title_prefix` 两次 `match self` — 可派生 `strum` / 手写一次。

### 1.7 `src-tauri/src/bot_model_loop.rs` (1852 行) — LLM 循环 + 流式 + Anthropic 适配

| 行号段 | 职责 | 抽出可命名 |
| --- | --- | --- |
| L1–488 | SSE chunk 解析 (`parse_sse_chunk` / `drain_sse_lines`) + tool delta 累积 + retryable 状态码 + fuse | `llm_stream.rs` |
| L518–end | `LlmHttp` / `ModelLoopDeps` + 主循环 | (留原文件) |

**Scissors-Slice 标记**:
- `mix-1` `bot_anthropic.rs` (1052 行) **本应是 bot_model_loop 的子模块**，却单独成文件；这是项目内多次"业务膨胀后 git mv 漏改"留下的痕迹。
- `mix-2` Anthropic 事件解析与 OpenAI SSE 解析在 bot_model_loop 中各占一片，应统一 `llm_stream.rs` 抽象 `EventStream` trait。

### 1.8 `src/components/SettingsPage.tsx` (1846 行) — **最大 React 文件**

按 `grep` 已能识别 7+ 协议/profile/migration/shortcut/keys 等面板全部堆在一个组件。

**Scissors-Slice 标记**:
- `mix-1` 设置项面板未拆；至少应该拆出 `ModelsPanel` / `SearchKeysPanel` / `ShortcutPanel` / `MigrationPanel`(已存在) / `ProfilePanel`(部分在)。
- `mix-2` 设置页用 `useState` 多到难以追踪 — 缺少 `useReducer` 或独立 hook 抽取。
- **对比**: 同目录 `MigrationPanel.tsx` 已独立成文件，说明**作者知道该分**，其余面板**没分**是工程债。

### 1.9 `src/components/ChatPanel.tsx` (1493 行)

| 块 | 职责 | 抽出可命名 |
| --- | --- | --- |
| 顶部 | `isImagePath` / `execute-task` 监听去重 / MarkdownText 包装 | `useExecuteTaskDedup.ts` |
| 中部 | 消息流渲染 + 滚动 + streaming | `ChatStream.tsx` |
| 尾部 | 工具行 + 输入框 | `ChatInput.tsx` |

**Scissors-Slice 标记**:
- `mix-1` 自定义 hook 缺位（与 `useInlineEdit.ts` 已抽形成对比）。
- `mix-2` `listen("execute-task")` 注册在模块级（注释里写了"模块级，跨组件实例/HMR 泄漏监听器共享"），但**没抽到独立 hook**，下一处复制粘贴概率高。

### 1.10 其余大文件清单（按行数降序，< 1500 行）

| 文件 | 行 | 切片建议 |
| --- | --- | --- |
| `bot_web.rs` (1434) | 搜索 + HTML 解析 + SSRF 防护 | 拆 `search.rs` / `html_parse.rs` / `ssrf.rs` |
| `profile.rs` (1077) | 头像 + 名称 | 拆 `profile_data.rs` / `profile_avatar.rs` / `profile_commands.rs` |
| `bot_anthropic.rs` (1052) | OpenAI→Anthropic 转换 | 并入 `bot_model_loop.rs` 子模块 `anthropic/` |
| `WidgetApp.tsx` (993) | 挂件 UI | 拆 `WidgetHeader.tsx` / `WidgetList.tsx` |
| `TodoCard.tsx` (972) | 任务卡 | 拆 `TodoCardHeader.tsx` / `TodoCardBody.tsx` / `TodoCardAttachments.tsx` |
| `bot_skills/scheduler.rs` (915) | DSL 编排 + rollback | `dsl_outcome.rs` / `dsl_rollback.rs` |
| `lib.rs` (862) | 入口 | 已是薄壳，OK |
| `bot_skills/runtime.rs` (826) | Skill 启动 + 步骤推进 | 拆 `skill_start.rs` / `skill_advance.rs` |
| `ocr.rs` (798) | 图片识字 | 拆 `ocr_macos.rs` / `ocr_windows.rs` / `ocr_core.rs` |
| `audit.rs` (739) | 审计日志 | 留原文件（单一职责） |
| `bot_fs.rs` (734) | 文件操作 | 拆 `fs_safe.rs` / `fs_command.rs` |
| `bot_scheduler.rs` (731) | 定时调度 | 留原文件 |

---

## 2. ponytail-audit 角度 — 全仓过度工程扫描

> ponytail-audit v1 格式: 每条一行 `<tag> <what>. <replacement>. [path:line]`，最后 `net: -<N> lines, -<M> deps possible.`

### 2.1 `delete:` 死代码 / 一次性使用 / 投机性抽象

| # | 位置 | 发现 | 替换 |
| --- | --- | --- | --- |
| 1 | `bot.rs:367–389` | `PermMode` enum (`yolo` / `ask` / `strict`) 在文件内仅 3 处读 + `perm_mode(app)` 单查询函数 — `yolo` 在 `bot_py.rs` L2136 用，`ask`/`strict` 仅默认 — 可能是 YAGNI 但有外部 spec，留意 | 暂无（spec 文档内提及） |
| 2 | `bot_skills/state.rs:196–272` | `test_hook_*` (3 个) + `test_*` (4 个) **双套测试入口**。`test_hook_insert_skill_run` / `test_insert_skill_run` 同形；`test_hook_remove_skill_run` / `test_remove_skill_run` 同形；`test_hook_skill_run_state` / `test_skill_run_state` 同形。 | 删 `test_hook_*` 三件套，统一走 `test_*`（pub(crate)） — `net: -45 lines` |
| 3 | `bot.rs:817–916` | `migrate_legacy_key` / `migrate_search_keys` / `migrate_search_key_slot` 三层 migrate — 历史迁移路径；当前版本可能已不再有"legacy 形态"用户。 | 如果线上无 legacy 用户 → 删 ; 否则保留 → 注释 deadline |
| 4 | `bot.rs:302–329` | `derive_default_label(base_url)` — 在 `derive_legacy_fields_from_active` 内单点调用，**其余 derive_* 系列也是单点**。`derive_*` 4 个函数合起来是一次性迁移脚本，可整段折叠到 `migrate_legacy_models` 内联。 | 内联 → `net: -45 lines` |
| 5 | `bot_py.rs:53–76` | `py_get_enabled` + `py_set_enabled` 两个 tauri command，但前端 `useEffect` 里通过 `invoke("py_get_enabled")` 调用频率极低（设一次永久生效）。 | 不动（spec 明确要求） |
| 6 | `api_handlers.rs:1155–1175` | `api_rotate_token` 极小函数 + 单点调用 | 不动（安全相关） |
| 7 | `lib.rs:50–56` (估) | `copy_file_with_title` 的 macOS / Windows / Other 三 cfg，`#[cfg(not(any(target_os = "macos", windows)))]` 分支会编译但运行时报 `DomainRule` 错误 — 第三个分支纯属占位（Linux 不发布）。 | 删第三个 cfg → 简化为二元。`net: -7 lines` |

### 2.2 `yagni:` 抽象只有一种实现

| # | 位置 | 发现 | 替换 |
| --- | --- | --- | --- |
| 1 | `bot.rs:64–96` | `KeySlot` enum（MainKey / SearchTavily / SearchBrave）+ `key_entry(slot)` + `read_key_of_slot` / `write_key_of_slot` / `has_key_of_slot` — 但 `KeySlot::MainKey` 走 `read_api_key` 旧路径，其余走 `search_key` 路径 — **两套并存**。 | 统一为单一 `KeySlot` + 一套 `read_key(slot)` / `write_key(slot, key)`。`net: -120 lines` |
| 2 | `bot_chat.rs:505–545` | `ChatGuard` + `bot_chat.rs:1093–1130` `ExecGuard` 两个 RAII 守卫抽象同形（Mutex + HashSet + Drop），**完全可以抽象为一个 `ReentrantSetGuard<String>`**。 | 抽 `ReentrantSetGuard` 到 `middleware.rs`，两个 Guard 都改成类型别名。`net: -50 lines` |
| 3 | `bot_chat.rs:1127–1155` | `TaskExecOrigin` 三个变体（Manual/Scheduled/Batch），每个有 `title_prefix()` + `as_str()` — 这是 `strum` 的标准用例。 | 加 `strum` 依赖 + `#[derive(Display, EnumString)]` → `net: -25 lines` (vs +依赖)。**不推荐**（+1 dep 不划算）；手写 `impl Display` 也行：`net: -15 lines`。 |
| 4 | `db.rs:496–517` | `reset_bot_assigned_with<F: FnOnce() -> Result<(), String>>` 单点调用 + 模板 callback — YAGNI 抽象 | 直接在唯一调用点写闭包 → `net: -20 lines` |
| 5 | `api_handlers.rs:825–873` | `register_sse_writer` + `stop_sse_writers` + `sse_connect` 抽象三件套，`stop_sse_writers` 是 `register_sse_writer` 的对称算子 — 可合并为 `SseHub` 结构。 | 抽 `SseHub { writers: Mutex<HashMap<usize, ...>> }`，封一个 `add/remove/broadcast`。`net: -30 lines` |

### 2.3 `stdlib:` 手写 stdlib 等价物

| # | 位置 | 发现 | 替换 |
| --- | --- | --- | --- |
| 1 | `bot.rs:441–464` | 3 个 cfg 变体的 `secret_service_available()` + 2 个 cfg 变体的 `linux_app_data_dir()` — 平台分支样板 | 抽 `cfg_secret_service_available()` 一次 + `cfg_unix!` 宏或 `#[cfg_attr]`，可压平（运行时不变）。`net: -25 lines` |
| 2 | `bot.rs:360–366` | `resolve_max_tokens(v: Option<u32>) -> u32` — 单字段默认包装 | `v.unwrap_or(0)` 在调用点直接展开 → `net: -7 lines` |
| 3 | `bot_chat.rs:159–235` | `truncate_split_point` / `truncate_chat_history` 双层函数 — 第二层只是循环调用第一层 | 单函数即可 → `net: -25 lines` |
| 4 | `bot_web.rs:439–565` | 自研 HTML 解析：`strip_tags` / `decode_entities` / `tag_block_span` / `remove_tag_blocks` / `find_ci` — 项目已依赖 `html2text = "0.13"` | **核心矛盾**：引 `html2text` 却不直接调，而是自己写 `extract_main_content`（L566–620）做近似事。**用 `html2text::from_read_with_decl(...)`** → 整段可删。`net: -120 lines` |
| 5 | `bot_web.rs:511` | `find_ci(haystack, needle, from)` — 大小写不敏感查找 | `haystack[from..].to_lowercase().find(&needle.to_lowercase())`（已经是 std）→ `net: -10 lines` |
| 6 | `bot_py.rs:1148–1166` | `dedup_dest(dir, name)` 同名加序号避让逻辑（doc_make_* 落 AI_Gen_Files 防覆盖） | **不该删**（spec 明确要求不覆盖），但 `claim_dst_name` 在 `migration.rs:576–612` 是相同逻辑的再实现 — 两处重复，应抽 `lib.rs::pick_non_colliding_path`。**复用而非删除** |
| 7 | `bot_skills/vars.rs:54–63` | `escape_json_str_inner` — 手写 JSON 字符串转义 | 项目已用 `serde_json::to_string(&v)` 多次，**转义本身无错**，但 `escape_for_log`/`escape_json_str_inner`/`truncate_for_log` 三个差不多函数散在多文件 | 抽 `escape_utils.rs` 集中。`net: -10 lines` |
| 8 | `lib.rs:115–170` | macOS `copy_file_macos` / Windows `copy_file_windows` 平台 helper — 必需，无法替换 | 留 |
| 9 | `api_handlers.rs:180–193` | 自研 `rate_check` 单文件 120/min 全局计数 | **不是 stdlib 等价物**，但**有 crate 替代**：`governor` 或 `tower-governor`。如果想加 worker 维度配额 → 引入；只想要单进程滑窗 → 留。 |

### 2.4 `native:` 平台/标准库已有能力

| # | 位置 | 发现 | 替换 |
| --- | --- | --- | --- |
| 1 | `bot_chat.rs:281–290` | `image_attach_indices(messages)` 走线性扫描 — `messages.iter().enumerate().filter(|(_, m)| m.attachments...)` 一行可替换 | `net: -10 lines` |
| 2 | `db.rs:981–1083` | `upsert_tasks` 长函数，循环 + 字段映射 + 冲突处理 | 单文件内有 6 个分支，可拆 `upsert_task_one(conn, task)` × N 调 — 不算 native，但属于"单函数承担太多" |
| 3 | `lib.rs:67–93` | `bring_main_to_front` `set_focus` + `set_always_on_top(true)` + `spawn` 等 80ms — Tauri 自身有 `request_user_attention` 等，但本项目想要的是"强制重排到 z-order 最顶"，确实是平台 workaround。 | 留 |

### 2.5 `shrink:` 同行逻辑写更多行

| # | 位置 | 发现 | 替换 |
| --- | --- | --- | --- |
| 1 | `migration.rs:720–1088` | `run_migration_inner` 内 `match rule.action` 三个 case（`move` / `delete` / 其他）每个都重复 "读 nt → 设 updated_at → 落盘 → commit journal → 记日志"。 | 抽 `mutate_and_commit(app, jconn, t, mutator: FnOnce(&mut Task), journal_id, log)`. `net: -120 lines` |
| 2 | `bot.rs:441–530` | `secret_service_available` 3 cfg 变体 + `linux_app_data_dir` 2 cfg 变体 + `warn_fallback_once` + `key_backend` — 平台分支树。 | `cfg_if::cfg_if!` 块或单 `cfg_secret_service_available()` 折成一次。`net: -35 lines` |
| 3 | `bot_chat.rs:546–897` | `bot_chat` / `bot_compact` / `bot_execute_task` / `bot_send` 等 6+ 个 `#[tauri::command]` 函数有相同的开头：开 `ChatGuard` → 调 `bot_get_enabled` → 错误处理。 | 抽 `command_prelude` async fn 返回 `Result<ChatGuard, CommandError>`，`#[tauri::command]` 直接 `let _g = command_prelude(&app, &session_id).await?;`。`net: -25 lines` |
| 4 | `bot_chat.rs:1162–1212` | `create_exec_session` 与 `persist_exec_reply` 都有 `tauri::async_runtime::spawn_blocking` + `DB_WRITE_LOCK.lock()` + `open_db` — 抽 `db_blocking<F, R>(app, f: F) -> Result<R, String>` | `net: -30 lines` |
| 5 | `bot_py.rs:2205–2400` | 6 个 `doc_*` 函数结构同形（拼 input JSON → 调 `run_doc_script` → 检查 exit_code → 记 audit → 返回路径）。 | 抽 `doc_command(name: &str, script: &str, input: Value, app: &AppHandle, what: &str) -> Result<String, CommandError>`. `net: -80 lines` |
| 6 | `SettingsPage.tsx:整个文件` | 多个 `useState` 关联字段无 reducer | `useReducer` 或抽出 `useSettingsForm` hook；至少能砍掉 5+ 个 useState |

### 2.6 Net 估算

```
delete:        -120 lines  (test_hook_*, derive_*, migrate_legacy_*)
yagni:         -160 lines  (KeySlot 二合一, ChatGuard 复用, TaskExecOrigin::strum-like)
stdlib:        -190 lines  (HTML 解析, find_ci, resolve_max_tokens, truncate_*)
native:        -10 lines   (image_attach_indices)
shrink:        -300 lines  (migration inner, doc_* 6 函数, command_prelude, etc)
─────────────────────────────────
net: -780 lines, -0 deps possible
```

**关键**: 这 -780 行几乎全是**改写**而非删除；删完后**对外 API 不变**（`bot_get_enabled` / `bot_chat` / `migration_status` 等签名保持），`tests-audit/audit_tauri_bridge.py` 应该全绿。

---

## 3. ponytail-review 角度 — 具体行号代码评审

> ponytail-review v1: `L<line>: <tag> <what>. <replacement>.` 或 `<file>:L<line>: ...` for multi-file。

### 3.1 bot.rs — 配置 + 凭据

```
bot.rs:441-449: yagni: 3 个 cfg 变体的 secret_service_available() 同语义实现. 单个 cfg_if! 块或 #[cfg_attr]，只编译时分支。  
bot.rs:484-490: yagni: 2 个 cfg 变体的 linux_app_data_dir() 同语义实现. 同样压平。  
bot.rs:64-96: yagni: KeySlot enum 两套 read/write/has 函数路径并存. 统一 KeySlot → 单套函数。  
bot.rs:302-329: yagni: derive_default_label() 单点调用. 内联到 migrate_legacy_models。  
bot.rs:817-916: yagni: migrate_legacy_key/migrate_search_keys/migrate_search_key_slot 三层嵌套 migrate. 折叠一层。  
bot.rs:360-366: stdlib: resolve_max_tokens(Option<u32>) -> u32. 1 行 v.unwrap_or(0)。  
```

### 3.2 bot_py.rs — Python 子进程 + 文档命令

```
bot_py.rs:2131-2205: shrink: py_exec_sync 多处"py_audit → run → py_audit"模板. 抽 helper py_audit_run(name, app, f).  
bot_py.rs:2205-2400: shrink: 6 个 doc_* 函数结构同形. 抽 doc_command(name, script, input, app, what).  
bot_py.rs:1151-1166: yagni: dedup_dest() 与 migration.rs:576-612 claim_dst_name() 同逻辑重复. 抽 lib::pick_non_colliding_path。  
bot_py.rs:30-46: stdlib: silent_cmd(program) L30 与 L40 同名重复定义 (不同 cfg). cfg 折叠为一个。  
```

### 3.3 db.rs — 数据层 + 命令

```
db.rs:1096-1209: shrink: load_all 单函数 113 行 + 字段映射. 抽 Task::from_row + load_all 主体。  
db.rs:703-927: shrink: 6 个 bot_sessions_* 命令都在文件底部 200 行内. 拆到 db_commands.rs。  
db.rs:496-517: yagni: reset_bot_assigned_with<F> 唯一调用点的回调闭包抽象. 直接内联闭包。  
db.rs:1209-1270: shrink: migrate_data_json + migrate_data_json_file 双层. 单函数即可。  
```

### 3.4 migration.rs — 文件迁移 + journal

```
migration.rs:720-1088: shrink: run_migration_inner 单函数 370 行 + 重复"set updated_at → upsert → commit journal → report"模板. 抽 mutate_and_commit helper。  
migration.rs:41-211: shrink: journal_pending/committed/cleared/find_pending 各有 _inner + lock 取锁样板. 抽 journal_db_blocking<F, R>。  
migration.rs:665-688: yagni: move_remove_fail_counts + record_move_remove_failure + move_remove_permanently_failed 三件套单点计数. 合并为一个 MoveRemoveTracker struct。  
migration.rs:714-720: shrink: emit_upserts(app, tasks) 单行包装. 调用方直接 app.emit(...) 即可。  
migration.rs:830-870 (估): shrink: 多个"decision match"分支先构造 nt 再落盘 — 每段都重复 nt.expected_updated_at = t.updated_at; nt.updated_at = Some(now);. 抽 Task::mutate_with_now(prev, now, f)。  
```

### 3.5 bot_chat.rs — 聊天编排

```
bot_chat.rs:505-545: yagni: ChatGuard RAII + bot_chat.rs:1093-1130 ExecGuard RAII 同形抽象. 抽 ReentrantSetGuard<String> 到 middleware.rs。  
bot_chat.rs:1127-1155: yagni: TaskExecOrigin 三变体 + title_prefix + as_str 两次 match. 手写 impl Display。  
bot_chat.rs:140-235: stdlib: truncate_split_point + truncate_chat_history 双层. 单函数。  
bot_chat.rs:1162-1212: shrink: create_exec_session 与 persist_exec_reply 都走 spawn_blocking + DB_WRITE_LOCK + open_db 模板. 抽 db_blocking<F, R>。  
bot_chat.rs:281-290: native: image_attach_indices 线性扫描. messages.iter().enumerate().filter(...).map(...).collect()。  
```

### 3.6 bot_web.rs — 联网工具

```
bot_web.rs:439-565: stdlib: 自研 HTML 解析（strip_tags / decode_entities / tag_block_span / remove_tag_blocks）. 改用 html2text::from_read_with_decl()。  
bot_web.rs:511-520: stdlib: find_ci 大小写不敏感查找手写. std 已有 to_lowercase() + find()。  
bot_web.rs:901-925: native: jina_reader_url + fetch_jina_reader 是第三方代理. 保留。  
bot_web.rs:941-998: shrink: ipv4_is_private + ipv6_is_private + parse_alt_ipv4 + is_private_host 4 个 SSRF 守卫函数. 可压成 1 个 fn is_private_socket(sock: SocketAddr) + is_private_host_str。  
```

### 3.7 bot_anthropic.rs — 协议转换

```
bot_anthropic.rs:41-340: shrink: openai_msgs_to_anthropic + convert_all + convert_content_blocks + tool_result_text + flush_tool_results + push_or_merge 全是 protocol convert. 抽 convert/mod.rs 集中。  
bot_anthropic.rs:412-525: native: parse_anthropic_event 手写 SSE event 解析. 与 bot_model_loop.rs:336-403 parse_sse_chunk 同形. 抽 llm::stream::Event trait 统一。  
```

### 3.8 api_handlers.rs — HTTP API

```
api_handlers.rs:180-193: stdlib (loose): 自研 rate_check 闭包. 简单场景可留；多维度配额场景建议 governor crate。  
api_handlers.rs:586-765: shrink: update_task 单函数 180 行 + 校验+合并+列派生+日志. 拆 validate + apply 两阶段。  
api_handlers.rs:825-873: yagni: register/stop sse_writer 三件套. 抽 SseHub struct。  
api_handlers.rs:416-585: shrink: create_task 单函数 170 行 + 字段映射. 同上拆 validate + apply。  
```

### 3.9 前端组件

```
SettingsPage.tsx:1-50: shrink: useState 串联字段（models / activeId / profiles / shortcuts ...）应 useReducer 或 useSettingsForm()。  
SettingsPage.tsx:整文件: yagni: 单文件 1846 行承担 7+ 面板职责. 拆 ModelsPanel / SearchKeysPanel / ShortcutPanel / ProfilePanel。  
ChatPanel.tsx:整文件: yagni: 1493 行混合消息流 + 输入框 + 执行去重 + 图片附件. 拆 ChatStream + ChatInput + useExecuteTaskDedup hook。  
TodoCard.tsx:972: yagni: 单文件承担卡片壳 + 折叠开关 + 文件绑定 + 子任务 + 行内编辑. 至少抽 TodoCardAttachments + useInlineEdit 已存在（说明作者知道该抽）。  
WidgetApp.tsx:993: yagni: 挂件整体. 至少拆 WidgetHeader + WidgetList + useWidgetDrag。  
src/lib/consts.ts:原生: 看 import 来源——前后端 IMAGE_EXTS 列表是否真有差异? 应单一来源 + 后端 consts::app_consts 下发（注释里有说明"真相在 bot_chat.rs::IMAGE_EXTS"——前端 consts.ts 复刻是冗余）。  
src/lib/errorHandler.ts:整文件: 多次 if isCommandError 链式 unwrap. 已经合理，不动。  
```

### 3.10 Net / 报告

```
net: -780 lines possible, -0 deps net (1 loose stdlib candidate: governor for rate limiting)
```

---

## 4. 历史审计沉淀 (已存在，不重复)

> 这部分在 `docs/AUDIT-*` (21 份) 已覆盖，本报告不重复。本节做的是**横向核对 + 留白**：

| 已审计 | 本报告新增 |
| --- | --- |
| `AUDIT-API-2026-09-03.md` / `AUDIT-CONTRACT-2026-08-28.md` | bot.rs KeySlot + db.rs `#[tauri::command]` 切分（**未覆盖**） |
| `AUDIT-BOT-PIPELINE-2026-08-27.md` | bot_chat.rs ChatGuard / ExecGuard 抽象合并（**未覆盖**） |
| `AUDIT-DATA-2026-08-28.md` | db.rs load_all 113 行（**未覆盖**） |
| `AUDIT-ERROR-2026-09-08.md` | error.rs 491 行未单独切（**未覆盖**） |
| `AUDIT-LLM-2026-08-28.md` | bot_anthropic.rs / bot_model_loop.rs SSE 双解析（**未覆盖**） |
| `AUDIT-SCHED-2026-08-28.md` | bot_scheduler.rs 731 行（**留白**） |
| `AUDIT-SECURITY-2026-08-27.md` | bot_web.rs 自研 HTML 解析 + SSRF（**部分覆盖**；HTML 解析冗余 vs html2text 是新发现） |
| `AUDIT-PLATFORM-2026-08-28.md` | lib.rs cfg 三分支占位（**未覆盖**） |
| `AUDIT-TESTS-2026-08-28.md` | bot_skills/state.rs `test_hook_*` + `test_*` 双套（**未覆盖**） |
| `AUDIT-SUMMARY-2026-08-28.md` | 阶段 5 规则固化 | ✓（`tests-audit/audit_*.py` 是其延续） |

**增量**: 本报告**新增覆盖 12 处**（标"未覆盖"项），与历史无矛盾，**全部正交**于已有规范。

---

## 5. 优先级建议（ROI 排序）

### 🔴 P0 — 立即可做（高 ROI、低风险）

1. **`migration.rs` `run_migration_inner` 370 行拆函数**（§1.4 + §3.4）— 抽 `mutate_and_commit` helper，可消除 ~120 行重复；改动集中在单文件内，对外行为不变。
2. **`bot_py.rs` 6 个 `doc_*` 函数抽 `doc_command` helper**（§1.1 + §3.2）— 抽公共结构，-80 行；产物落 AI_Gen_Files 路径完全不变。
3. **`bot.rs` KeySlot 双路径合一**（§2.2 #1 + §3.1）— 抽 `KeySlot::read/write/has/del` 统一函数，-120 行；API key 与 search key 共用同一套读写。

### 🟡 P1 — 中期（高 ROI、中风险 — 需回归测试）

4. **`SettingsPage.tsx` 拆 4 个子面板**（§1.8 + §3.9）— 1846 → ~500 行主文件 + 4 子组件。
5. **`ChatPanel.tsx` 拆 ChatStream + ChatInput + useExecuteTaskDedup hook**（§1.9 + §3.9）— 1493 → ~400 行。
6. **`bot_chat.rs` ChatGuard / ExecGuard 合并为 ReentrantSetGuard**（§2.2 #2 + §3.5）— 抽到 `middleware.rs`。
7. **`db.rs` `#[tauri::command]` 全部下移到 `db_commands.rs`**（§1.3 + §3.3）— 解耦 DB 核心与命令绑定。
8. **`bot_web.rs` 自研 HTML 解析改用 `html2text::from_read_with_decl`**（§2.3 #4 + §3.6）— 注意：现有 `extract_main_content` 保留视觉重点段落，**不是 1:1 等价**，迁移需要 A/B 验证抓取质量。

### 🟢 P2 — 长期（低 ROI / 维护性）

9. **`bot_skills/state.rs` 删 `test_hook_*` 三件套**（§2.1 #2）— 单点回归。
10. **`bot.rs` cfg 折叠**（`secret_service_available` 3 cfg → 1 cfg、`linux_app_data_dir` 2 cfg → 1 cfg）— 编译时分支简化。
11. **`api_handlers.rs` `update_task` / `create_task` 单函数拆 validate + apply**（§3.8）— 200+180 行拆 4 函数。
12. **`bot_anthropic.rs` 与 `bot_model_loop.rs` SSE 解析统一 `EventStream` trait**（§3.7）— 需要更多测试覆盖。

### ⛔ 不推荐

- **引入 `strum`** for `TaskExecOrigin` — 1 dep 换 -15 行不划算。
- **引入 `governor`** for `rate_check` — 除非要加 worker 维度，否则单进程滑窗足够。
- **删 `tests-audit/` 两个 pytest** — 它们是阶段 5 规则固化的产物，砍了 P0 级门禁会松。

---

## 6. 验证清单（推荐执行顺序）

> 仅建议**如何验证切片后行为不变**，不强制执行。

```bash
# 1. 基线回归
cd /Users/renshi/Projects/wmessage
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-fail-fast
# 期望: test result: ok. 472 passed（基线 audit_pre_step_pre_execute.py::test_cargo_test_lib_baseline 锁住 400）

# 2. 命令/事件一致性门禁（阶段 5 规则）
python3 -m pytest tests-audit/audit_tauri_bridge.py -v
python3 -m pytest tests-audit/audit_pre_step_pre_execute.py -v

# 3. 切片后行为不变验证
# - bot_get_config / bot_set_config 序列化字段不变 → snapshot diff
# - migration_status 返回结构不变 → snapshot diff
# - doc_make_word 返回路径不变 → fixture 比对
# - api /tasks GET 返回 JSON 字段不变 → snapshot diff

# 4. 前端无回归
npm run test
npm run build
```

---

## 7. 约束与免责

- 本报告**未触碰任何代码、文件结构、Cargo.toml 依赖**。
- 所有行号引用基于 `wc -l` + `grep -nE '^(pub |async )?fn|^impl|^pub struct|^pub enum|^#\[tauri::command\]'` 在 2026-09-12 09:49 GMT+8 的实际快照。
- ponytail-audit 的 net: -780 行是**改写估算**，实际执行需配合回归测试覆盖。
- Scissors-Slice 标记的所有"建议切片"都是**方向**，不是**承诺**；实际拆分请用 git worktree 单独跑分支。
- 历史 21 份 `docs/AUDIT-*` 报告的结论**不重复审计**；本报告对其只做横向核对（§4）。

---

*报告生成于 2026-09-12 09:49 GMT+8，使用工具组合: Scissors-Slice (v1.1.0) + ponytail-audit + ponytail-review。*