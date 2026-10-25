# 大文件拆分计划（2026-10-25）

来源：用户提问「这 18 条要不一起拆掉」。本文是**执行计划**，不是已完成记录——
落地进度按批次回填。

## 〇、一句话结论

能拆，但不是「18 个文件各拆一次」那么均质。这 18 条债务的**性质差异极大**，
分成三类：

| 类别 | 条数 | 债务总量 | 拆分手法 | 风险 |
|---|---|---|---|---|
| **A 类·测试外移** | 8 | ~10,400 行 | 把内联 `#[cfg(test)] mod` 整块搬到 `tests.rs` 子模块 | 极低 |
| **B 类·生产代码切分** | 7 | ~15,700 行 | 按职责切成子模块，facade 保持旧路径 | 中 |
| **C 类·特殊形态** | 3 | ~12,600 行 | 集成测试 crate / 门禁自身 / 工具单源 | 中高 |

**A 类先做**（当天可完成 8 条，零行为变化），**B 类按依赖序串行**，
**C 类最后做**（每条都要单独设计，不适合批量）。

---

## 一、实测基线（2026-10-25）

行数口径 = 换行符个数（与 `wc -l` 同值），与 `tests-audit/audit_file_size.py`
一致。白名单 18 条全部处于「登记值 == 实测值」的棘轮紧位——**即任何一次提交
里让这些文件变大都会当场 fail**。

「生产 / 内联测试」的切法：文件里第一个 `#[cfg(test)]` 之前算生产，之后算
内联测试。注意这个切法与 `audit.rs::freeform_freeze` 的扫描口径**同源**
（它也按首个 `#[cfg(test)]` 切生产切片），所以下面的数字对两处口径都成立。

| 文件 | 登记上限 | 实测 | 生产 | 内联测试 | 类别 |
|---|---|---|---|---|---|
| `tests/llm_integration.rs` | 4645 | 4645 | 4646 | 0 | C |
| `src/bot/registry.rs` | 3827 | 3827 | 2546 | 1282 | B |
| `src/db/trace.rs` | 3040 | 3040 | 1652 | 1389 | B |
| `src/bot_chat.rs` | 3028 | 3028 | 2764 | 265 | B |
| `src/bot/tools.rs` | 2949 | 2949 | 2323 | 627 | B |
| `src/bot_fs.rs` | 2846 | 2846 | 1875 | 972 | B |
| `src/db/mod.rs` | 2468 | 2468 | 409 | 2060 | A |
| `src/db/tasks.rs` | 2467 | 2467 | 1513 | 955 | B |
| `src/py/document.rs` | 2456 | 2456 | 2142 | 315 | B |
| `src/bot/services.rs` | 2391 | 2391 | 2389 | 3 | B |
| `src/bot/mcp/manager.rs` | 2075 | 2075 | 1213 | 863 | A |
| `src/bot_web.rs` | 2050 | 2050 | 1400 | 651 | A |
| `src/apps/manage.rs` | 1884 | 1884 | 966 | 919 | A |
| `src/bot/dispatch.rs` | 1819 | 1819 | 1526 | 294 | B |
| `src/audit.rs` | 1749 | 1749 | 715 | 1035 | A |
| `src/bot_skills/manage.rs` | 1664 | 1664 | 1506 | 159 | A |
| `src/bot_scheduler.rs` | 1617 | 1617 | 1162 | 456 | A |
| `src/bot_anthropic.rs` | 1524 | 1524 | 765 | 760 | A |

合计 38,697 行。其中 **内联测试 12,866 行（33%）**——这部分是「假重量」：
它不占生产逻辑，但照样把文件顶到超限线上。**先把它挪走，8 个文件直接
达标**。

---

## 二、三条硬约束（拆之前必须知道）

### 2.1 行数门禁是棘轮，不是提醒

`tests-audit/audit_file_size.py`：

- 新文件一律 ≤ 1500 行（`test_new_files_respect_size_limit`）
- 白名单内文件**只许变小**——比登记值大一行就 fail
- 文件降到 ≤ 1500 → **白名单条目必须整条销掉**，否则
  `test_whitelist_has_no_stale_entries` fail
- 任何 > 1500 行的新文件没登记 → `test_whitelist_covers_every_over_limit_file` fail

推论：**每拆完一个文件，同一提交内必须更新白名单登记值**。拆完不回填 =
下一个 commit 直接红。这条是「拆文件」这件事最常见的翻车点。

### 2.2 模块地图对拍：新文件必须进文档两处

`tests-audit/audit_module_map.py` 要求每个新 `.rs` 同时出现在：

1. `docs/rust-bot-architecture.md` §6.4「源文件清单」——漏登记直接 fail
2. 文档开头的模块树（`── xxx.rs` 条目）——且带 `(NNN)` 行数标注的条目，
   行数必须与实际**精确一致**（`test_annotated_line_counts_match_files`）
3. 文档开头的散文数字「共 N 个文件、约 M 万行」必须与实测一致

也就是说：拆文件不是纯代码改动，**每个批次都要同步改文档**（模块树 +
§6.4 清单 + 散文数字三处）。这三处对不上，门禁一样红。

### 2.3 命名空间守卫：bot 族外不许摸深层路径

`tests-audit/audit_bot_namespace.py`：族外文件不得出现
`crate::bot_[a-z0-9_]+::`。bot 族 = `bot.rs` + `bot/` 目录 + `bot_*.rs` +
`bot_*/` 目录族。

推论：拆 `src/bot_chat.rs` 成 `src/bot_chat/` 目录后仍属族内（`bot_` 前缀），
自由互引；但若拆出 `src/chat/` 这类非 `bot_` 前缀目录，**族外引用必须改走
`crate::bot::` 门面**。本计划统一用 `bot_` 前缀 + 目录，避免这个坑。

---

## 三、执行计划（12 批）

### 批次总表

| 批 | 内容 | 类别 | 触及文件 | 预计减债 | 风险 | 可否并行 |
|---|---|---|---|---|---|---|
| 1 | `db/mod.rs` 测试外移 | A | 1 | ~2060 | 极低 | ✅ |
| 2 | `audit.rs` 测试外移 | A | 1 | ~1035 | 极低 | ✅ |
| 3 | `bot/mcp/manager.rs` 测试外移 | A | 1 | ~863 | 极低 | ✅ |
| 4 | `apps/manage.rs` 测试外移 | A | 1 | ~919 | 低 | ✅ |
| 5 | `bot_anthropic.rs` + `bot_web.rs` + `bot_scheduler.rs` 测试外移 | A | 3 | ~1867 | 极低 | ✅ |
| 6 | `bot_skills/manage.rs` 测试外移 + 生产切分 | A+B | 1 | ~160+ | 低 | ✅ |
| 7 | `db/tasks.rs` 生产切分 | B | 1 | ~1500 | 中 | ✅ |
| 8 | `db/trace.rs` 生产切分 | B | 1 | ~1650 | 中 | ✅ |
| 9 | `bot_fs.rs` 生产切分 | B | 1 | ~1880 | 中 | ✅ |
| 10 | `bot/tools.rs` + `bot/services.rs` 生产切分 | B | 2 | ~3000 | 中高 | ⚠️ 串行 |
| 11 | `bot_chat.rs` + `bot/registry.rs` 生产切分 | B | 2 | ~3300 | 高 | ⚠️ 串行 |
| 12 | `tests/llm_integration.rs` 拆分 | C | 1 | ~4650 | 中高 | ⚠️ 串行 |

**批次 1–6 合计减债 ~8,400 行**，18 → 12 条（A 类 8 条全部清账）。

### 批次 1 · `src/db/mod.rs`（2468 → ~410）

生产代码只有 409 行，**2060 行全是内联测试**。本批是 18 条里性价比最高的。

拆法：

```
src/db/mod.rs (410)        模块声明 + re-export + open_db + ensure_columns
src/db/tests.rs (30)       #[cfg(test)] mod tests; 的门面（沿用 bot_model_loop 范式）
src/db/tests/open_db.rs    原 mod tests（1463 行）——需再切一刀，见下
src/db/tests/ws.rs         原 mod ws_tests（416）
src/db/tests/reset.rs      原 mod reset_tests（70）
src/db/tests/atomic.rs     原 mod atomic_write_tests（111）
```

注意 `mod tests` 本体 1463 行**超过 1500 上限**——按主题再切成
`open_db.rs` / `schema.rs` / `workspace.rs` 三块。

风险点：`db/mod.rs` 顶部 `pub use crate::db::tasks::*` 等 13 条 re-export
是**跨模块公开面**，切分时必须原样保留（`crate::db::Task` 这类路径全仓
在用）。`open_db` 247 行是核心，保持在 mod.rs。

### 批次 2 · `src/audit.rs`（1749 → ~715）

```
src/audit.rs (715)         AuditLevel / write_event / 日志写入 / 错误分类
src/audit/tests.rs (30)    测试门面
src/audit/tests/classify.rs    原 mod error_class_tests
src/audit/tests/core.rs        原 mod tests（896）
src/audit/tests/freeze.rs      原 mod freeform_freeze（85）
```

**本批的关键陷阱**：`mod freeform_freeze` 是**门禁自身**——它扫描
`src/**/*.rs`，按「首个 `#[cfg(test)]` 之前的切片」找 `audit_log(` 调用，
并对照 33 条路径白名单。拆到 `src/audit/tests/freeze.rs` 后：

- 它自身的扫描范围不变（扫 `src/` 全树），但**它自己这个文件的路径变了**
- 白名单里没有 `audit/tests/freeze.rs` → 它若含 `audit_log(` 字面量会被自己判 fail
  （实测：`audit.rs` 全文 3 处 `audit_log(`，但切片内 0 处——因为 3 处都在
  `#[cfg(test)]` 之后，守卫扫不到。拆出去后新文件**没有** `#[cfg(test)]` 标记
  在前，切片会覆盖全文 → **新文件会被自己判 fail**）

处置：新文件顶部加 `#[cfg(test)]`（或把扫描逻辑改为按文件名跳过 `tests/` 目录）。
**推荐后者**——治本，且未来所有 `tests.rs` 拆出文件都免疫。改完顺手把
`src/bot_fs.rs` 那 20 处（当前因首个 `#[cfg(test)]` 在第 75 行而被漏掉的）
也纳入视野，或明确记为「有意放行」。

### 批次 3 · `src/bot/mcp/manager.rs`（2075 → ~1215）

```
src/bot/mcp/manager.rs (1215)   McpManager 本体 + impl + fingerprint
src/bot/mcp/manager/tests.rs (30)
src/bot/mcp/manager/tests/probe.rs       原 mod send_probe
src/bot/mcp/manager/tests/fingerprint.rs 原 mod fingerprint_tests
src/bot/mcp/manager/tests/gate.rs        原 mod http_gate
src/bot/mcp/manager/tests/e2e.rs         原 mod e2e_stdio（672）
```

本文件已有一个非标准位置的 `#[cfg(test)]`：`SHARED_MCP_TEST_LOCK`
（第 288 行，在 impl 之前）。**它被 `registry.rs` 的测试引用**——
`manager.rs:283` 注释明说「registry 侧断言拿同一把锁」。拆到子模块后
这个 `pub(crate)` static 的可见性要保留（`pub(crate)` 即可，跨模块可见）。

`impl McpManager` 本体 861 行（312-1173）——单 impl 超过 800 行，
Rust 允许，但拆分时可考虑按职责拆成 `connect.rs` / `call.rs` / `retire.rs`
三个 `impl McpManager` 块（同一类型可有多处 impl，Rust 允许）。

### 批次 4 · `src/apps/manage.rs`（1884 → ~970）

```
src/apps/manage.rs (970)
src/apps/manage/tests.rs (30)
src/apps/manage/tests/core.rs (890)   原 mod tests（919）
```

**注意**：`apps/manage.rs` 在 `audit.rs::freeform_freeze` 白名单里
（4 处 `audit_log(`）。拆出 `apps/manage/tests.rs` 后：

- 白名单路径 `apps/manage.rs` 仍有效（主文件还在，路径未变）✅
- 但新增的 `apps/manage/tests/core.rs` 若含 `audit_log(` → 需加白名单
  （实测 `apps/manage.rs` 4 处全在切片内；测试块内 0 处，安全）

### 批次 5 · 三个文件的测试外移

| 文件 | 拆分后 | 测试去向 |
|---|---|---|
| `src/bot_anthropic.rs` (1524→765) | `src/bot_anthropic/tests.rs` + `tests/core.rs` | 原 `mod tests`（760） |
| `src/bot_web.rs` (2050→1400) | `src/bot_web/tests.rs` + `tests/core.rs` | 原 `mod tests`（651） |
| `src/bot_scheduler.rs` (1617→1162) | `src/bot_scheduler/tests.rs` + `tests/sched.rs` / `tests/guard.rs` / `tests/plan.rs` | 原 3 个 mod（456） |

`bot_scheduler.rs` 在冻结白名单里（18 处 `audit_log(`，全在切片内）——
主文件路径不变，白名单仍有效。

### 批次 6 · `src/bot_skills/manage.rs`（1664 → ~1230）

```
src/bot_skills/manage.rs (~800)   SkillInfo / 扫描 / 启停 / 导入 / 删除
src/bot_skills/manage/create.rs (~400)  create_skill_at + tool_create_skill + validate_created_meta
src/bot_skills/manage/tests.rs (30)
src/bot_skills/manage/tests/core.rs (160)
```

生产 1506 行 → 光挪测试不够（1506 仍 > 1500），需把
`create_skill_at`（135）+ `tool_create_skill`（151）单独切出。

### 批次 7 · `src/db/tasks.rs`（2467 → ~1050）

生产 1513 行，切点清晰：

```
src/db/tasks.rs (~800)      Task/Subtask/TaskFile 类型 + upsert + db_* 命令
src/db/tasks/patch.rs (~450) apply_task_patch（217 行）+ task_patch_locked + task_set_column
src/db/tasks/transfer.rs (~350)  export/import（check_export_path / export_tasks_json /
                                 import_tasks_conn / tasks_export / tasks_import）
src/db/tasks/tests.rs (30)
src/db/tasks/tests/patch.rs (836)  原 mod task_patch_tests
src/db/tasks/tests/column.rs (120) 原 mod task_set_column_tests
```

**跨模块耦合点**：`apply_task_patch` 里改任务字段后要广播 `tasks-updated`
事件，依赖 `crate::mutation::MutationOrigin` 与 `bot::tools::broadcast_after_mutation`。
切开后需显式 `use` 导入，否则编译器报错（这是好事——编译器会指出所有漏接的依赖）。

`db/mod.rs` 的 `pub use crate::db::tasks::*` 需要同步：子模块的公开项要
在 `tasks.rs` 门面 re-export 出来，保持 `crate::db::Task` 路径不变。

### 批次 8 · `src/db/trace.rs`（3040 → ~1650）

```
src/db/trace.rs (~700)      类型定义 + trace_start/finish + span/file_change 写入
src/db/trace/query.rs (~450) trace_query / spans_for_trace / session_tool_names / trace_list
src/db/trace/usage.rs (~400) usage_stats_* 四个函数 + 保留期清理
src/db/trace/rollback.rs (~450) file_rollback + trace_export + session file changes
src/db/trace/tests.rs (30)
src/db/trace/tests/core.rs (1070)
src/db/trace/tests/session.rs (318)
```

**本文件有 9 个 `#[tauri::command]`**（`trace_list` / `trace_detail` /
`trace_clear_before` / `usage_stats_*` / `file_rollback` /
`bot_session_file_changes` / `trace_export`）。Tauri 命令注册在 `lib.rs`
走 `crate::db::trace::xxx` 路径。切分后**必须保持 `crate::db::trace::xxx`
仍可达**——办法是 `trace.rs` 门面 re-export（`pub use query::*;`），
`lib.rs` 一行不改。

这一点是整个计划的**通用手法**：**所有被 `lib.rs` 注册的命令一律留在门面
文件可见**，子模块只做实现搬移。`audit_tauri_bridge.py` 双向对拍会自动
兜住漏项。

### 批次 9 · `src/bot_fs.rs`（2846 → ~1500）

```
src/bot_fs.rs (~450)        allowed_dirs / resolve_with_perm / writable_dirs / lookup_tool_rule
src/bot_fs/edit.rs (~700)   try_apply_edit + edit_file_sync + tool_edit_file + tool_write_file
src/bot_fs/read.rs (~750)   tool_read_text_file + tool_grep_files + tool_list_files + inode 检查
src/bot_fs/receipt.rs (~200) FileChangeReceipt + build_unified_diff + build_file_change_receipt
src/bot_fs/tests.rs (30)
src/bot_fs/tests/core.rs (680)
src/bot_fs/tests/change.rs (180)
src/bot_fs/tests/read.rs (65)
```

**本文件有 20 处 `audit_log(`，但因首个 `#[cfg(test)]` 在第 75 行
（`mod tool_rule_tests`），守卫切片内是 0 处——当前是「靠位置侥幸躲过」。**
拆分后新文件没有前置 `#[cfg(test)]`，20 处全部暴露 → 必须把新路径加入
冻结白名单，或先做 `audit_log(` → `audit_event!` 迁移。

**建议**：本批顺手把 `bot_fs.rs` 的 `audit_log(` 迁成 `audit_event!`
（批 5 的整改方案已在做同一件事，方向一致），迁完就不必改白名单。

### 批次 10 · `src/bot/tools.rs` + `src/bot/services.rs`

**`bot/tools.rs`（2949 → ~1450）**，生产 2323 行，切点：

```
src/bot/tools.rs (~350)      公共类型 + TaskView + render_task_line + 工具入口分发
src/bot/tools/task.rs (~700)  query_tasks / query_single_task / create / complete / delete / edit / subtask
src/bot/tools/doc.rs (~600)   extract_document + create_word/excel/ppt/pdf
src/bot/tools/service.rs (~500) start/stop/list_service
src/bot/tools/tests.rs (30)
src/bot/tools/tests/*.rs (6 块，共 627)
```

**`bot/services.rs`（2391 → ~1200）**，注意它已有子目录 `src/bot/services/`
（`watchdog.rs` + `tests.rs`），是**模块转目录**的现成范式：

```
src/bot/services.rs (~700)   ManagedService / ServiceState / do_start / stop_entry
src/bot/services/gate.rs (~600) 端口/监听闸（judge_sockets / judge_flags / enforce_listening_gate）
src/bot/services/watchdog.rs (295, 已有)
src/bot/services/tests.rs (1487, 已有)
```

`services.rs` 现有 `#[cfg(test)] mod tests;` 指向 `services/tests.rs`（1487 行），
已经在用子模块范式——**这一条是 A 类手法的现成样板**，照抄即可。

`bot/tools.rs` 有 8 处 `audit_log(`，在冻结白名单里，主文件路径不变则白名单有效。

### 批次 11 · `src/bot_chat.rs` + `src/bot/registry.rs`

**`bot_chat.rs`（3028 → ~1400）**：

```
src/bot_chat.rs (~500)       入口 + 类型 + 命令
src/bot_chat/flow.rs (~700)   bot_chat / bot_chat_inner（460 行主体）
src/bot_chat/exec.rs (~700)   bot_execute_task / run_task_in_chat_with / ExecGuard
src/bot_chat/media.rs (~250)  attach_images / image_part_from_file
src/bot_chat/summary.rs (~350) truncate_with_summary / summarize_http
src/bot_chat/tests.rs (30)
src/bot_chat/tests/*.rs (4 块，共 265)
```

**`bot/registry.rs`（3827 → ~1500）**：这条最特殊——它是**工具单源真相**
（`TOOLS_TABLE` + 63 个 schema 常量 + `claims_mutation` 守卫判据），
全仓 172 处 `crate::bot::registry::` 引用，且被 `bot.rs` 门面 re-export。

```
src/bot/registry.rs (~250)    类型（ToolCtx/ToolResult/ToolDef）+ 门面 re-export
src/bot/registry/schema.rs (~320)  63 个 SCHEMA_* 常量
src/bot/registry/table.rs (~640)  TOOLS_TABLE + assemble_tools_json
src/bot/registry/claims.rs (~620) claims_mutation + 判据（EXISTENCE_CLAIM_MARKERS 等）
src/bot/registry/tests.rs (30)
src/bot/registry/tests/*.rs (4 块，共 1282)
```

**手法约束**：`src/bot/registry.rs` 已是 `pub mod registry;`（`bot.rs:100`），
拆成目录后 `bot/registry/` 目录 + `bot/registry.rs` 门面共存——Rust 2018
支持这种「同名 .rs + 目录」结构（`bot/procs.rs` + `bot/procs/` 已是先例）。

**门面必须逐项 re-export**：`bot.rs` 门面转出 `crate::bot::registry::X` 的
条目，加上 172 处直接 `crate::bot::registry::` 引用，**一个都不能少**。
`audit_bot_namespace.py` 的 `test_facade_rejects_direct_deep_use` 禁止
`pub use crate::bot_xxx::*` 整模块透传——所以必须逐项列。

### 批次 12 · `tests/llm_integration.rs`（4645 → 3 块）

**集成测试 crate**，与前 11 批性质不同：它在 `tests/` 下，是独立 crate，
**不能有 `mod` 子目录**（Cargo 的 `tests/*.rs` 每个是独立 target，
子目录里的文件不会被自动编译）。三条路：

1. **拆成多个 `tests/*.rs`**：Cargo 自动把 `tests/` 下每个 `.rs` 当独立
   测试 target。代价：91 个用例共享的 `CoreHarness`（364-515 行）得复制
   或挪进 `tests/mock_llm.rs` 这类共享文件。
2. **保持单文件、把测试函数按主题分文件后 `include!`**：仓库已有
   `include!("mock_llm.rs")` 的先例（6 个测试文件都这么干）。沿用此法：
   ```
   tests/llm_integration.rs (~600)   入口 + mock_llm_re_export + 共享 harness
   tests/llm_integration/core.rs     core_* 用例
   tests/llm_integration/guard.rs    幻觉守卫 / 声称对账用例
   tests/llm_integration/anthropic.rs anthropic 专用用例
   ```
   `include!` 会把内容内联进同一 crate，共享 harness 直接可见——
   **这是本批的正确解法**。
3. 改用 `tests/llm_integration/main.rs` 目录形态：Cargo 支持
   `tests/<name>/main.rs` 作为独立 target。**最干净**，但会改 `cargo test
   --test llm_integration` 的路径（仍可用，target 名不变）。

推荐 **方案 3**：`tests/llm_integration/main.rs` + 同目录 `core.rs` /
`guard.rs` / `anthropic.rs`。注意 `mock_llm.rs` 被 6 个测试文件 `include!`，
**不能移动**（会被其他文件打断）——留在 `tests/` 根，走
`include!("../mock_llm.rs")`。

---

## 四、每批的固定收尾清单（缺一不可）

每个批次 = 一个 commit，提交前逐项确认：

```
□ cargo fmt --check
□ cargo clippy --all-targets -- -D warnings   （含「测试压根没被收集」信号）
□ cargo nextest run                            （Rust 全量）
□ npm test                                     （前端全量）
□ python3 -m pytest tests-audit/audit_file_size.py -q        ← 行数棘轮
□ python3 -m pytest tests-audit/audit_module_map.py -q       ← 模块地图
□ python3 -m pytest tests-audit/audit_bot_namespace.py -q    ← 命名空间
□ python3 -m pytest tests-audit/audit_tauri_bridge.py -q     ← Tauri 桥
□ python3 -m pytest tests-audit/audit_doc_claims.py -q       ← 事件名对拍
□ python3 tests-audit/audit_no_eval.py
□ 更新 tests-audit/audit_file_size.py 的 WHITELIST 登记值（拆完变小 → 改小；
   降到 ≤1500 → 整条删）
□ 更新 docs/rust-bot-architecture.md 三处：模块树条目(+行数标注) / §6.4 清单 /
   开头散文「共 N 个文件、约 M 万行」
□ 人眼过一遍 diff：出现任何非「纯搬移」的行即回退
```

**「纯搬移」的判定标准**：diff 里除了 `use` 增删、`mod` 声明、可见性修饰，
不应出现任何逻辑行改动。出现即为「顺手重写」，回退重来。

---

## 五、并行策略

批次 1–6（A 类）触及文件互不相交，**可开 3 个并行会话**：

- 会话 A：批次 1、2（db/mod.rs、audit.rs）
- 会话 B：批次 3、4（mcp/manager.rs、apps/manage.rs）
- 会话 C：批次 5、6（bot_anthropic、bot_web、bot_scheduler、bot_skills/manage）

**并行前置条件**（否则会互相打架）：

1. 各会话用**独立 git worktree**，不共享工作区
   （`git worktree add ../wm-split-a -b split/a`）
2. 合并时**文档三处（模块树 / §6.4 / 散文数字）只能由一个会话最终统一改**
   ——三处都是全局单值，并行改必冲突。
   **处置**：并行会话只改代码 + 白名单登记值；文档三处留到最后一批由主会话
   统一改。代价是中间批次的 `audit_module_map.py` 会红——**这是可接受的**，
   合并时一次性补齐即过。
3. 白名单 `WHITELIST` 字典同样是全局单值 → 同上，合并时统一改

**批次 7–12 全部串行**：`bot/registry.rs`（172 处引用）、`bot_chat.rs`、
`bot/tools.rs`、`bot/services.rs` 之间耦合紧，且都碰 `lib.rs` 的命令注册面。

---

## 六、风险登记

| 风险 | 影响 | 处置 |
|---|---|---|
| 拆完不回填白名单 | 下个 commit 门禁红 | 收尾清单固定项，拆完同 commit 改 |
| 新文件漏登 §6.4 | `audit_module_map` fail | 同上；文档三处最后统一改 |
| `#[cfg(test)]` 位置变化暴露 `audit_log(` | 冻结守卫 fail | 批次 2 改守卫口径（按目录跳过 tests/）；批次 9 顺带迁移 bot_fs |
| 子模块可见性收窄 | 编译错 | 编译器兜住，但注意 `pub(crate)` / `pub(super)` 在下移一层后含义变化（`pub(super)` 在子模块里指向子模块的父级——**必须改成 `pub(crate)` 或显式 `pub(in crate::...)`**） |
| `lib.rs` 命令路径失效 | Tauri 桥 fail | 门面 re-export 保持路径；桥门禁双向对拍兜底 |
| `db/mod.rs` 的 13 条 `pub use` 漏转 | 全仓编译错 | 编译器兜底；`crate::db::Task` 等路径零改动是硬要求 |
| `registry.rs` 门面整模块透传 | `audit_bot_namespace` fail | 必须逐项 `pub use`，禁 `pub use crate::bot_xxx::*` |
| 并行会话改文档/白名单冲突 | 合并冲突 | 并行期禁改这两处，合并期统一改 |
| 拆分中「顺手优化」 | 行为漂移 | 纯搬移红线 + 人眼 diff |

**已识别的 `pub(super)` 陷阱**：`bot_skills/manage.rs` 现有
`pub(super) fn data_dir_subdir`（第 57 行）。拆成 `bot_skills/manage/` 子模块后，
`pub(super)` 的父级从 `bot_skills` 变成 `bot_skills::manage`——**可见性收窄，
调用方可能编译不过**。必须显式改 `pub(in crate::bot_skills)` 或 `pub(crate)`。

---

## 七、不做清单

- 不改门禁结构（只改 `audit.rs` 的冻结扫描口径一处，且是修 bug 不是加规则）
- 不顺手重构：拆分过程中发现的所有「代码味道」记入 DEVLOG，不在拆分的 commit 里改
- 不动前端（`.ts/.tsx` 不在本批范围；`SettingsPage.tsx` 4068 行是另一件事）
- 不合并批次 1–6 的 commit（每个文件一个 commit，回退粒度细）
- 不改 `TOOLS_TABLE` 的条目顺序与 schema 字面量（工具单源，任何改动都要单独立项）
- 不出 Windows 包（AGENTS.md 既有口径：修完先让用户在现有环境过一遍）

---

## 八、进度回填

| 批 | 状态 | commit | 白名单登记值变更 | 备注 |
|---|---|---|---|---|
| 1 | ⬜ 未开始 | | | |
| 2 | ⬜ 未开始 | | | 含冻结守卫口径修复 |
| 3 | ⬜ 未开始 | | | |
| 4 | ⬜ 未开始 | | | |
| 5 | ⬜ 未开始 | | | 3 文件 |
| 6 | ⬜ 未开始 | | | |
| 7 | ⬜ 未开始 | | | |
| 8 | ⬜ 未开始 | | | 9 个 tauri 命令 |
| 9 | ⬜ 未开始 | | | 顺带迁 audit_log |
| 10 | ⬜ 未开始 | | | |
| 11 | ⬜ 未开始 | | | 172 处引用，最险 |
| 12 | ⬜ 未开始 | | | 集成测试 crate |
