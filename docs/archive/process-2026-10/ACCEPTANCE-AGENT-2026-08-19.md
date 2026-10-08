# WMessage Agent 代码手工验收单

> **版本**：2026-08-19 · 对应 `HEAD = 03ec76d`（Phase 1–7b 完成态）
> **用途**：人工逐项验收 AI Agent 编排层代码的正确性、边界条件、状态机、错误流
> **范围**：`bot_chat` / `bot_model_loop` / `bot_scheduler` / `bot_slash` / `bot_skills` / `bot_py` / `middleware` / `intent_router` / `tool_guard` / `audit` / `api_*` / 前端 agent UI
> **前置条件**：参见 §0 通过后再验 §1–§8

---

## §0 预检（5 分钟）

### 0.1 编译与测试基线

```bash
# macOS
cd /Users/renshi/Projects/wmessage
export PATH="/Users/renshi/.cargo/bin:$PATH"

# Rust 编译
cargo build --lib 2>&1 | grep -E "(error|warning: unused|warning:.*dead)" | grep -v "pre-existing"
# 期望：0 error；warnings 可接受（见 §0.2）

# 单元测试（不含前端）
cargo test --lib 2>&1 | tail -5
# 期望：344+ pass，0 fail（或仅 bot_skills debug-dir 2 个 pre-existing fail）

# TypeScript 编译
cd src
npx tsc --noEmit 2>&1 | tail -3
# 期望：0 error，0 warning

# 前端测试
cd ..
npx vitest run 2>&1 | tail -8
# 期望：61+ pass，0 fail
```

### 0.2 已知 pre-existing warnings（可忽略）

| 文件 | 警告类型 | 原因 |
|---|---|---|
| `migration.rs:219` | `unused import: crate::db::Task` | Phase 7b P2-4 遗留，未影响 |
| `migration.rs:47` | `dead_code: fields state/created_at never read` | JournalEntry 结构预留字段，未启用 |

### 0.3 审计日志文件基线

```bash
# 审计日志应在 db_dir() 下，格式验证：
# 每行格式：[ts.毫秒] LEVEL | event | k=v | k=v ...
# 7 个 P0 事件名：user.message / tool.call / tool.return / skill.start /
#                 llm.request / llm.response / chat_execute.routed
head -20 "$(find ~/Library/Application\ Support/com.wmessage /Users/renshi/.local/share/wmessage /Users/renshi/Projects/wmessage/src-tauri/target/debug/deps -name 'bot.log' 2>/dev/null | head -1)" 2>/dev/null || echo "bot.log 未生成（正常，首次触发后产生）"
```

---

## §1 代码块关联表

> 说明：「依赖」列 = 该代码块依赖的其他模块；「被调用」列 = 谁调用它。关联表顺序 = 推荐验收顺序（底层先验）。

### A 层：基础设施（无业务依赖）

| # | 模块 | 文件 | 关键代码块 | 依赖 | 被调用 |
|---|---|---|---|---|---|
| A1 | **审计子系统** | `src/audit.rs` | `write_event` / `write_error_audit` / `escape_for_log` / `append_line` / `build_event_line` / `BOT_LOG_LOCK` / `AuditLevel` / `probe_log_dir` | 无 | bot_chat / bot_model_loop / execute_tool / middleware / api / bot_py |
| A2 | **错误体系** | `src/error.rs` | `CommandError` 枚举（含 `BotDisabled`/`TaskInvalidState`/`IO_ERROR`/`Network`/`Internal`/`KeyringError` 等） / `Platform` / `hintForCode` / `is_recoverable` / `from_str_recoverable` | 无 | 所有返回 `CommandResult` 的 command |

### B 层：安全策略（依赖基础设施）

| # | 模块 | 文件 | 关键代码块 | 依赖 | 被调用 |
|---|---|---|---|---|---|
| B1 | **中间件注册表** | `src/middleware.rs` | `MiddlewareRegistry` / `run_pre_step` / `run_pre_execute` / `IntentRouterMiddleware` | A1 audit | execute_tool / bot_model_loop |
| B2 | **意图路由（L1 硬锁）** | `src/intent_router.rs` | `INTENT_RULES`（7 条规则）/ `route_user_input` / `start_skill` | A1 audit / B1 middleware | bot_chat 入口 |
| B3 | **原子工具门卫** | `src/tool_guard.rs` | `is_atomic_tool` / `ATOMIC_BLACKLIST`（2 项）/ `ATOMIC_WHITELIST`（18 项） | A1 audit / B1 middleware | execute_tool |

### C 层：编排层（依赖 B 层）

| # | 模块 | 文件 | 关键代码块 | 依赖 | 被调用 |
|---|---|---|---|---|---|
| C1 | **LLM 流式循环** | `src/bot_model_loop.rs` | `run_model_loop` / `parse_sse_chunk` / `feed_think` | A1 audit / B1 B2 B3 | bot_chat |
| C2 | **聊天入口** | `src/bot_chat.rs` | `bot_chat` / `require_bot_enabled` / `execute_task_core` / `ExecGuard` / `StopGuard` | A1 A2 / B1 B2 B3 / C1 | api / 前端 |
| C3 | **定时调度器** | `src/bot_scheduler.rs` | `schedule_mission` / `check_due_tasks` / `advance_skill` | A1 A2 / B1 B2 B3 / C2 | lib.rs（启动时） |
| C4 | **开关管理** | `src/bot_slash.rs` | `bot_get_enabled` / `bot_set_enabled` / `bot-flag-path` | A2 | bot_chat / execute_task |

### D 层：Skill 子系统（依赖 C 层）

| # | 模块 | 文件 | 关键代码块 | 依赖 | 被调用 |
|---|---|---|---|---|---|
| D1 | **DSL 解析器** | `src/bot_skills.rs` | `parse_step_heading` / `parse_tool_call` / `strip_frontmatter` / `parse_skill_steps` | A1 audit | D2 / D3 |
| D2 | **变量替换** | `src/bot_skills.rs` | `substitute_vars` / `extract_task_id` / `CompletedStep` | D1 | D3 |
| D3 | **调度执行器** | `src/bot_skills.rs` | `run_skill_scheduler` / `AdvanceAction` / `advance_skill` / `SkillRun` | A1 A2 / B1 B3 / D1 D2 / C2 | execute_tool / bot_chat |
| D4 | **Skill 清单扫描** | `src/bot_skills.rs` | `scan_skill_dirs` / `scan_all_skills_in_debug_dir` | A1 audit | lib.rs（启动时） |

### E 层：高危工具（独立依赖）

| # | 模块 | 文件 | 关键代码块 | 依赖 | 被调用 |
|---|---|---|---|---|---|
| E1 | **Python 沙箱** | `src/bot_py.rs` | `py_get_enabled` / `py_set_enabled` / `detect_python` / `run_python` / `run_python_at` / `drain_output` / `RunLimits` / `StopToken` / `py_audit` / `PY_RUN_GATE` | A1 A2 / B3 | execute_tool |
| E2 | **EPHEMERAL 资源** | `src/api_handlers.rs` | `api_stop` / `rotate_token` / SSE writer lifecycle | A1 | lib.rs |

### F 层：API 外部接口

| # | 模块 | 文件 | 关键代码块 | 依赖 | 被调用 |
|---|---|---|---|---|---|
| F1 | **API 鉴权** | `src/api_auth.rs` | `verify_bearer` / `ct_eq`（恒定时间比较）/ `write_token_file` | A1 A2 | F2 |
| F2 | **API 命令处理器** | `src/api_handlers.rs` | `handle_request` / `change_log_line` / `tasks_export` / `tasks_import` / `query_single_task` / `bot_chat`（API 版） | A1 A2 / F1 | 网络请求 |
| F3 | **API 服务器** | `src/api_server.rs` | `start_api` / `stop_api` / SSE `broadcast_changes` / `EventHub` | A1 A2 / F2 | lib.rs（启动时） |

### G 层：前端 Agent UI

| # | 模块 | 文件 | 关键代码块 | 依赖 | 被调用 |
|---|---|---|---|---|---|
| G1 | **错误展示** | `src/frontend/src/agent/errorHandler.ts` | `handleCommandError` / `formatCommandError` / `hintForCode` | — | storage.ts / App.tsx |
| G2 | **流式响应** | `src/frontend/src/agent/useAgentChat.ts` | `onDelta` / `bot-chat-delta` 事件消费 | — | ChatPage |
| G3 | **停止机制** | `src/frontend/src/agent/useStopController.ts` | `/stop` 请求 / `stop` 状态 | — | ChatPage |
| G4 | **Skill 执行状态** | `src/frontend/src/agent/useSkillStatus.ts` | skill-running / skill-finished 事件消费 | — | AgentPanel |

---

## §2 验收顺序与依赖图

```
A1 audit ──────────────┐
A2 error ──────────────┤
                        ├──→ B1 middleware ──→ B2 intent_router ──→ C2 bot_chat ──→ C3 scheduler
                        │          │                    │                │
                        │          └──→ B3 tool_guard ──→ execute_tool ──┤
                        │                                               │
                        └──────────────────────────────────────────────→ C1 model_loop
                                                                              │
D1 DSL 解析 ──→ D2 变量替换 ──→ D3 调度器 ←────────────────────────────┘
     ↑                    ↑                  ↑
     └────────────────────┘                  │
           D4 Skill 扫描                     │
                                             │
E1 Python 沙箱 ←────────────────────────────┘
E2 API 资源 ←─────────────────────────────────┘
F1 鉴权 ←─┐
           ↓
F2 handlers ←──→ F3 api_server
G1 错误 UI ←─┐
G2 流式   ←─┤
G3 停止    ←─┤
G4 Skill  ──┘
```

**验收顺序规则**：
1. **先基础设施（A1/A2）**：audit 和 error 是所有模块的依赖，错了会级联
2. **再安全层（B1/B2/B3）**：防护链错了业务层全暴露
3. **再业务编排（C1/C2/C3/C4）**：主循环、主入口、调度器
4. **再 Skill 子系统（D1/D2/D3/D4）**：依赖 C 层
5. **再高危工具（E1/E2）**：Python 沙箱 + 资源清理
6. **再 API（F1/F2/F3）**：外部接口
7. **最后前端（G1–G4）**：消费上述所有

---

## §3 详细验收条目

> 每个条目格式：`[ID] 检查项` → 步骤 → 通过标准 → 失败定位

---

### §3.1 A1 — 审计子系统

#### [A1-1] 事件命名空间合规

**步骤**：
```bash
grep -E "audit_event!\(|write_event\(" src/audit.rs | grep -v "//" | head -20
```
对照 §1 A1 的 `write_event` 调用点（bot_chat / execute_tool / middleware / api / bot_py），找每个调用点的 `event` 参数。

**通过标准**：7 个 P0 事件名严格匹配（大小写敏感）：
- `user.message`
- `tool.call`
- `tool.return`
- `skill.start`
- `llm.request`
- `llm.response`
- `chat_execute.routed`

**额外新增**（Phase 6a D2/D3 授权）：
- `middleware_registry_missing`
- `profile_load_fail`

**失败定位**：audit.rs `write_event` 调用点逐一检查 event 字符串字面量。

---

#### [A1-2] `escape_for_log` 防注入

**步骤**：
```bash
grep -rn "escape_for_log" src/ --include="*.rs" | grep -v "test\|//"
```
确认所有写入审计日志的文本（title / preview / kv 值 / 命令输出）全部经过 `escape_for_log`。

**通过标准**：
- `\n` → `\\n`（防伪造日志行）
- `| ` → `||`（防伪造 kv 分隔符）
- 截断至 300 字符

**测试验证**：
```bash
cargo test --lib escape_for_log 2>&1 | tail -5
# 期望：test audit::tests::kv_value_pipe_space_and_newline_stay_escaped ... ok
```

---

#### [A1-3] `write_event` 写失败不 panic

**步骤**：读 `src/audit.rs` 的 `append_line` 函数。

**通过标准**：
- `append_line` 返回 `bool`
- open / write 失败 → `eprintln!` 带 path → 返回 `false`（不 panic）
- `write_event` 和 `write_error_audit` 共用 `append_line`

**测试验证**：
```bash
cargo test --lib append_line 2>&1 | tail -5
# 期望：有测「只读目录写入失败 → false + eprintln」用例
```

---

#### [A1-4] `BOT_LOG_LOCK` 全局串行

**步骤**：读 `src/audit.rs` 顶部 `BOT_LOG_LOCK` 声明及所有 `let _ = BOT_LOG_LOCK.lock()` 调用点。

**通过标准**：
- `BOT_LOG_LOCK` 是 `Mutex<()>`
- 所有 `write_event` / `write_error_audit` / `audit_log` 调用前必须 `let _ = BOT_LOG_LOCK.lock()`
- `bot.log` 是顺序写的，无交叉污染

---

#### [A1-5] `probe_log_dir` 三分支

**步骤**：
```bash
cargo test --lib probe_dir 2>&1 | tail -10
```
**通过标准**：
- 分支 1：`current_exe` parent 可写 → 返回 exe 目录
- 分支 2：exe 目录只读 → 退 app_data_dir
- 分支 3：皆不可用 → `temp_dir()`
- `db::data_dir` / `profile::data_dir` / `generic_log_dir` 全部调用 `probe_log_dir`

---

### §3.2 A2 — 错误体系（只读验证，不改）

> ⚠️ `error.rs` 不可修改（MEMORY.md 硬约束）。以下为只读验证。

#### [A2-1] `CommandError` 变体齐全

**步骤**：
```bash
grep -E "^    [A-Z][a-zA-Z]+|" src/error.rs | head -30
```

**通过标准**（至少包含）：
```
BotDisabled        # bot 总开关关闭
TaskInvalidState   # 业务状态拒绝（P2-28 新增）
IO_ERROR           # IO 错误（带 Platform）
Network            # 网络错误
Internal           # 通用内错
KeyringError       # 密钥链错误
PythonNotFound     # Python 未找到
TimeOut            # 超时
```

#### [A2-2] `Platform` 跨平台区分

**步骤**：
```bash
grep -rn "Platform" src/error.rs src/api_handlers.rs | grep -v "//\|test"
```

**通过标准**：
- `Platform` 枚举含 `Windows / Macos / Linux`
- `IO_ERROR` 序列化时带 `"platform": "macos"` / `"linux"` 字段
- 同一 `code` 在不同平台可区分

---

### §3.3 B1 — 中间件注册表

#### [B1-1] 注册顺序强制

**步骤**：读 `src/middleware.rs` 的 `MiddlewareRegistry::register` 及 `lib.rs` 中间件注册调用顺序。

**通过标准**：
- `IntentRouterMiddleware` 必须在列表**最后**注册
- `SafetyMiddleware` 或等价防护中间件在**最前**
- `D1` 测试断言顺序：`assert!(middlewares[0].is_safety()); assert!(middlewares.last().is_intent_router());`

**测试验证**：
```bash
cargo test --lib middleware_order 2>&1 | tail -5
```

---

#### [B1-2] `run_pre_step` / `run_pre_execute` panic 兜底

**步骤**：读 `src/middleware.rs` 的 `run_pre_*` 实现。

**通过标准**：
- `catch_unwind(AssertUnwindSafe(|| ...))()`
- panic → 写 `middleware_panic` ERROR 审计 → 返回 `None`
- 主流程不挂，继续放行

**测试验证**：
```bash
cargo test --lib catch_unwind 2>&1 | tail -5
```

---

#### [B1-3] 空注册表 → `pre_execute_not_registered` 审计

**步骤**：读 `src/middleware.rs` 的 `run_pre_execute`。

**通过标准**：
- 注册表为空时 → 写 `pre_execute_not_registered` 审计（Warn 级）→ 返回 `None`
- 不 panic，不阻断

---

### §3.4 B2 — 意图路由

#### [B2-1] 7 条 INTENTS 规则

**步骤**：
```bash
grep -A 2 "INTENT_RULES" src/intent_router.rs | head -30
```

**通过标准**：7 条规则覆盖以下意图关键词（大小写不敏感）：
| # | 关键词（部分） | Skill |
|---|---|---|
| 1 | ppt / 幻灯片 / 演示文稿 | `ppt-orchestra-skill` |
| 2 | docx / word / 文档修订 | `minimax-docx` |
| 3 | xlsx / excel / 电子表格 | `minimax-xlsx` |
| 4 | pdf | `minimax-pdf` |
| 5 | 联网 / 搜索 / web | `minimax-web-search` |
| 6 | 任务汇总 / 归档 | `minimax-task-summary` / `minimax-archive` |
| 7 | python / 脚本 | （白名单，intent_router 不拦）|

---

#### [B2-2] 路由命中 → Skill 启动

**步骤**：读 `src/intent_router.rs` 的 `route_user_input` 返回值处理。

**通过标准**：
- 命中 → 返回 `(SkillMeta, body)` 元组
- `body`（DSL SKILL.md 内容）被拼入 `bot_chat` 的 system prompt
- `bot_chat` 对 `mode == "auto"` 的 Skill 调用 `run_skill_scheduler`（跳过 LLM）

---

### §3.5 B3 — 工具门卫

#### [B3-1] 黑名单 2 项

**步骤**：
```bash
grep -A 5 "ATOMIC_BLACKLIST" src/tool_guard.rs
```

**通过标准**：
- `create_word_revisions`
- `link_file_to_task`

**黑名单拦截**：非 Skill Running 状态 → `pre_execute.deny` 审计 → 返回拒绝提示。

---

#### [B3-2] 白名单 18 项

**步骤**：
```bash
grep -A 25 "ATOMIC_WHITELIST" src/tool_guard.rs
```

**通过标准**：18 个 function 可直接调用（不走 Skill 调度）：
```
list_tasks / create_task / complete_task / delete_task / edit_task /
add_subtask / toggle_subtask / bind_file / search_tasks /
extract_document / create_word / create_excel / create_ppt / create_pdf /
run_python / web_search / fetch_url / use_skill
```

---

### §3.6 C1 — LLM 流式循环

#### [C1-1] SSE chunk 解析稳定

**步骤**：
```bash
cargo test --lib parse_sse 2>&1 | tail -10
```

**通过标准**：
- 多 chunk 粘包正确合并
- `data: ` 前缀去除
- `event:` 行解析到对应字段
- `done` 信号识别

---

#### [C1-2] `feed_think` 写 think 内容

**步骤**：读 `src/bot_model_loop.rs` 的 `feed_think` 函数。

**通过标准**：
- LLM think 块内容被捕获并写入 bot.log（通过 `audit_event!`）
- 不丢失，不截断（除非超长）

---

### §3.7 C2 — 聊天入口

#### [C2-1] `bot_get_enabled == false` → `BotDisabled`

**步骤**：
```bash
cargo test --lib bot_disabled 2>&1 | tail -5
```

**通过标准**：
- `require_bot_enabled(false)` → `Err(CommandError::BotDisabled)`
- `CommandError::BotDisabled.code()` → `"BOT_DISABLED"`
- `is_recoverable()` → `true`（引导用户去设置页）

---

#### [C2-2] `ExecGuard` 防重入

**步骤**：读 `src/bot_chat.rs` 的 `ExecGuard` 实现（RAII）。

**通过标准**：
- 同一 `task_id` 同时只能有一个 `ExecGuard`
- 第二个获取 → `Err(CommandError::Internal(...))`
- 函数返回 / panic → 自动释放

---

#### [C2-3] 三个执行入口全部走 middleware

**步骤**：读 `src/bot_chat.rs` 三个入口：
1. `bot_chat`（chat-mode LLM 循环）
2. `execute_task`（task-mode 执行）
3. slash 命令执行（`bot_slash.rs`）

**通过标准**：
- 三入口均调用 `require_bot_enabled`（总开关）
- 均经过 `run_pre_execute`（工具预检）
- 均经过 `run_pre_step`（意图路由）

---

### §3.8 C3 — 定时调度器

#### [C3-1] `advance_skill` 实时超时检测

**步骤**：
```bash
cargo test --lib advance_skill 2>&1 | tail -15
```

**通过标准**（6 个状态 × 5 个分支）：
- `NoActive` / `AwaitConfirm` / `Continue` / `Finish` / `Fail` / `Terminate`
- Running 状态恰好 60s → `Fail("timeout")`
- Running 状态超时 61s → 立即返回 `Fail`（不等 step_check）

---

### §3.9 C4 — 开关管理

#### [C4-1] `bot-enabled.flag` 文件语义

**步骤**：
```bash
cargo test --lib bot_get_enabled 2>&1 | tail -5
```

**通过标准**：
- `bot_get_enabled` → `flag_path.exists()`
- `bot_set_enabled(true)` → 创建文件（`b"1"`）
- `bot_set_enabled(false)` → 删除文件
- 文件在 `db_dir()`（exe 同目录或 app_data_dir）

---

### §3.10 D1 — DSL 解析器

#### [D1-1] `parse_skill_steps` 正确解析格式

**步骤**：
```bash
cargo test --lib parse_skill 2>&1 | tail -10
```

**通过标准**：
- `## Step N: 标题` → `SkillStep { index: N, title, tool_name, args_json }`
- `tool_name({"arg": "value"})` → 提取 `tool_name` 和 `args_json`
- `## Rollback` 段 → 独立解析，不混入 Steps
- frontmatter（`---` 之间的 YAML）→ 被 `strip_frontmatter` 剔除

---

#### [D1-2] `substitute_vars` 4 种变量

**步骤**：
```bash
cargo test --lib substitute_vars 2>&1 | tail -20
```

**通过标准**：
- `${step1.id}` → 替换为第一步提取的 UUID
- `${step1.result}` → 替换为第一步的返回文本
- `${prev.id}` / `${prev.result}` → 指向上一步（ctx 末尾）
- 越界索引 / 未知 field → 保留原文 `${...}` 不变

---

### §3.11 D2 — Skill 调度执行器

#### [D2-1] `run_skill_scheduler` 顺序执行

**步骤**：
```bash
cargo test --lib run_skill_scheduler 2>&1 | tail -10
```

**通过标准**：
- 按 `steps` 顺序执行
- 每步执行完 → push `CompletedStep` 到 ctx
- 变量替换在每步执行前完成
- Rollback 段在失败时执行

---

#### [D2-2] `SkillRun` 状态持久化

**步骤**：读 `src/bot_skills.rs` 的 `active_skill_run()` 和 `SkillRun` 结构。

**通过标准**：
- `active_skill_run: Option<SkillRun>` 在内存持久
- `SkillRun` 含：`name` / `steps` / `ctx` / `started_at` / `mode`
- `advance_skill` 修改状态但不丢失 ctx

---

### §3.12 E1 — Python 沙箱

#### [E1-1] `py_get_enabled == false` → 直接拒绝

**步骤**：
```bash
grep -B 2 -A 5 "py_get_enabled" src/bot_py.rs | grep -v "//"
```

**通过标准**：`run_python` 入口第一行：
```rust
if !py_get_enabled(app.clone()) {
    return Err("Python 编程未开启：请到设置页「机器人设置」打开「允许机器人执行 Python」".into());
}
```

---

#### [E1-2] `RunLimits` 强制超时 300s

**步骤**：读 `src/bot_py.rs` 的 `MAX_TIMEOUT_SECS` 和 `RunLimits::terminate`。

**通过标准**：
- `MAX_TIMEOUT_SECS = 300`
- 用户请求 `timeout_secs > 300` → 直接拒绝（不传到底层）
- 底层 `run_python` 另有 `timeout` 钳制（双保险）

---

#### [E1-3] 子进程独立进程组 + `kill_tree`

**步骤**：读 `src/bot_py.rs` 的 `run_python_at` → `Command::group()` / `kill_tree`。

**通过标准**：
- Unix：`Command::group()` 创建独立进程组
- `kill_tree` 杀整组（含孙进程）
- Windows：Job Object 限制子进程
- Reader thread 2s timeout 兜底（`drain_timeout` 审计出现）

---

#### [E1-4] `/stop` 取消 `StopToken`

**步骤**：
```bash
grep -rn "StopToken\|stop_token\|StopGuard" src/bot_py.rs | grep -v "//\|test" | head -15
```

**通过标准**：
- `StopToken` = `Arc<AtomicBool>`
- `run_python` 循环内每次 `recv` 前检查 `stop.is_stopped()`
- `/stop` 发送 → `stop.store(true)` → `recv` 返回 `Disconnected` → 退出循环 → `kill_tree`

---

#### [E1-5] `PY_RUN_GATE` 并发互斥

**步骤**：`PY_RUN_GATE = Mutex::new(())` + `let _guard = PY_RUN_GATE.lock().unwrap()`。

**通过标准**：
- 同一时刻只允许一个 Python 子进程
- 第二个请求 → 等锁释放后执行（不拒绝）

---

### §3.13 E2 — API 资源生命周期

#### [E2-1] SSE writer `api_stop` 通知

**步骤**：读 `src/api_handlers.rs` 的 `api_stop` 实现。

**通过标准**：
- `api_stop` 写事件到 SSE 管道 → SSE writer 收到 → 优雅退出
- 5s 兜底 join（`api_server.rs` `drop` 时触发）
- 泄漏 `ERROR` 审计出现

---

### §3.14 F1 — API 鉴权

#### [F1-1] `ct_eq` 恒定时间比较

**步骤**：
```bash
cargo test --lib ct_eq 2>&1 | tail -10
```

**通过标准**：
- 手写 XOR 折叠（无 `subtle` crate）
- 耗时与错误位置无关（timing attack 防护）
- 测试覆盖：`ct_eq("same", "same")` → true；`ct_eq("same", "diff")` → false

---

#### [F1-2] token 文件权限 0600

**步骤**：
```bash
grep -A 10 "write_token_file\|api-token" src/api_auth.rs | head -20
```

**通过标准**：
- `std::fs::write` 后 → `fs::set_permissions(path, Mode(0o600))`
- Unix 下 chmod 生效

---

### §3.15 F2 — API 命令处理器

#### [F2-1] 变更日志 `task.title` 防注入

**步骤**：`change_log_line` 函数 + `escape_for_log`。

**通过标准**：`task.title` 过 `escape_for_log` 后再拼接进日志行；`\n` / `| ` 被转义。

---

#### [F2-2] `TaskInvalidState` 业务拒绝

**步骤**：读 `src/bot_chat.rs` 三处业务拒绝（执行中 / 已完成 / 已归档）。

**通过标准**：
- 返回 `Err(CommandError::TaskInvalidState { reason })`
- `code()` → `"TASK_INVALID_STATE"`
- `is_recoverable()` → `true`

---

### §3.16 G1 — 前端错误展示

#### [G1-1] 空 msg 兜底

**步骤**：读 `src/frontend/src/agent/errorHandler.ts` 的 `handleCommandError`。

**通过标准**：
- `CommandError` 空 message → `e.code || "未知错误"`
- 非结构化空 msg → `alert("❌ 未知错误")`
- 不再跳过 / 弹空白 alert

---

#### [G1-2] recoverable + `onRetry` → 重试按钮

**步骤**：读 `errorHandler.ts` 的 confirm 分支。

**通过标准**：
- `is_recoverable == true` + 有 `onRetry` 回调 → `confirm(...)` 对话框含「重试」意图
- `confirm` 文案包含「重试」

---

#### [G1-3] 写路径空 catch 清理

**步骤**：
```bash
grep -n "\.catch(() => {})" src/frontend/src/agent/storage.ts | head -10
grep -n "\.catch(() => {})" src/frontend/src/App.tsx | head -5
```

**通过标准**：
- `storage.ts` 写操作（`upsertWorkspaceItems` / `saveRules` / `exportTasks` 等）**全部**走 `handleCommandError`
- `App.tsx` 的 `upsertWorkspaceItems` catch 不再静默（有 `console.error` 记录或 `silent: true` 明确标记）

---

### §3.17 G2 — 流式响应

#### [G2-1] `bot-chat-delta` 事件消费

**步骤**：读 `src/frontend/src/agent/useAgentChat.ts` 的 `onDelta` 处理。

**通过标准**：
- SSE `bot-chat-delta` 事件 → 增量渲染到 chat 区域
- 流式打字效果（逐字或逐 token）
- `bot-chat-finished` 事件 → 结束流式

---

### §3.18 G3 — 停止机制

#### [G3-1] `/stop` 按钮中断 LLM 循环

**步骤**：读 `src/frontend/src/agent/useStopController.ts`。

**通过标准**：
- 点击 `/stop` → `POST /api/stop`（或前端直接发停止事件）
- LLM 流式中断（`bot-chat-delta` 停止推送）
- UI 显示「已停止」

---

## §4 跨模块集成验收（全链路）

> 以下验收需完整端到端，不能只验单个模块。

### [INT-1] 用户消息 → Skill 执行全链路

**路径**：`用户输入` → `bot_chat` → `intent_router` → `run_skill_scheduler` → `execute_tool` → `bot_py::run_python` → 响应

**验证步骤**：
1. 前端发送包含 Skill 关键词的消息（如"帮我做一个 PPT"）
2. `intent_router` 命中 → `mode=auto`
3. `run_skill_scheduler` 执行 DSL steps
4. `execute_tool` 调用 `run_python`
5. Python 执行结果通过 SSE 推送回前端
6. 全程审计日志正确记录：`user.message` / `pre_step.route_skill` / `tool.call` / `tool.return` / `skill.start`

**通过标准**：无 panic / 无 500 / 审计日志 7 个事件齐全

---

### [INT-2] `/stop` 中断在途 Python

**路径**：`/stop` → `StopToken.store(true)` → `recv` 返回 → `kill_tree` → SSE 通知前端

**验证步骤**：
1. 发送耗时 Python 脚本（`time.sleep(30)`）
2. 3s 后点击 `/stop`
3. 观察：Python 子进程被杀死（进程树无残留）
4. SSE writer 收到通知并退出（`api_stop` 审计出现）
5. 审计日志：`tool.call` / `tool.return`（超时/取消）

**通过标准**：无僵尸进程；reader thread 在 2s 内退出

---

### [INT-3] bot 开关关闭 → 明确拒绝

**路径**：`bot_chat` → `require_bot_enabled(false)` → `CommandError::BotDisabled`

**验证步骤**：
1. 删除 `bot-enabled.flag`（或 `bot_set_enabled(false)`）
2. 发送任意消息
3. 观察：返回 `BOT_DISABLED` 错误，前端弹窗引导去设置页

**通过标准**：不是 `INTERNAL` / 不是空白无响应

---

### [INT-4] Python 开关关闭 → 明确拒绝

**路径**：`run_python` → `py_get_enabled(false)` → Err

**验证步骤**：
1. 删除 `py-enabled.flag`
2. 发送"帮我写个 Python 脚本"
3. 观察：Skill 调度执行到 `run_python` 时返回 `Python 编程未开启`

**通过标准**：不是崩溃 / 不是静默无响应

---

### [INT-5] 高并发任务卡编辑

**路径**：多个客户端同时拖拽排序 / 编辑同一任务卡

**验证步骤**：
1. 打开两个客户端（模拟两个设备）
2. 同时拖拽同一任务卡
3. 观察：无数据竞争 / 无死锁 / 最后状态一致

**通过标准**：`DB_WRITE_LOCK` 保护无写冲突；前端 diff 规则正确合并

---

## §5 签收

| 检查项 | 结果 | 检查人 | 日期 |
|---|---|---|---|
| §3 A1–A2（基础设施） | ✅ / ❌ | | |
| §3 B1–B3（安全策略） | ✅ / ❌ | | |
| §3 C1–C4（业务编排） | ✅ / ❌ | | |
| §3 D1–D2（Skill 子系统） | ✅ / ❌ | | |
| §3 E1–E2（高危工具） | ✅ / ❌ | | |
| §3 F1–F2（API） | ✅ / ❌ | | |
| §3 G1–G3（前端） | ✅ / ❌ | | |
| §4 INT-1–INT-5（集成） | ✅ / ❌ | | |
| 编译基线（§0） | ✅ / ❌ | | |

**备注**：

---

*本文档由小九（OpenClaw Agent）生成 · 2026-08-19*
*对应代码：`03ec76d`（Phase 1–7b 完成态）*
