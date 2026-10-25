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

| 文件 | 登记上限 | 实测 | 生产 | 内联测试 | 类别 | 落地 |
|---|---|---|---|---|---|---|
| `tests/llm_integration.rs` | 4645 | 4645 | 4646 | 0 | C | ✅ 目录形态 6 文件 |
| `src/bot/registry.rs` | 3827 | 3827 | 2546 | 1282 | B | ⬜ 未开始 |
| `src/db/trace.rs` | 3040 | 3040 | 1652 | 1389 | B | ✅ 908 |
| `src/bot_chat.rs` | 3028 | 3028 | 2764 | 265 | B | ✅ 419（门面） |
| `src/bot/tools.rs` | 2949 | 2949 | 2323 | 627 | B | ✅ 88（门面） |
| `src/bot_fs.rs` | 2846 | 2846 | 1875 | 972 | B | ✅ 465（门面） |
| `src/db/mod.rs` | 2468 | 2468 | 409 | 2060 | A | ✅ 416 |
| `src/db/tasks.rs` | 2467 | 2467 | 1513 | 955 | B | ✅ 958 |
| `src/py/document.rs` | 2456 | 2456 | 2142 | 315 | B | ✅ 1097 |
| `src/bot/services.rs` | 2391 | 2391 | 2389 | 3 | B | ✅ 1332（门面） |
| `src/bot/mcp/manager.rs` | 2075 | 2075 | 1213 | 863 | A | ✅ 1216 |
| `src/bot_web.rs` | 2050 | 2050 | 1400 | 651 | A | ✅ 1400 |
| `src/apps/manage.rs` | 1884 | 1884 | 966 | 919 | A | ✅ 966 |
| `src/bot/dispatch.rs` | 1819 | 1819 | 1526 | 294 | B | ✅ 1009 |
| `src/audit.rs` | 1749 | 1749 | 715 | 1035 | A | ✅ 717 |
| `src/bot_skills/manage.rs` | 1664 | 1664 | 1506 | 159 | A | ✅ 625 |
| `src/bot_scheduler.rs` | 1617 | 1617 | 1162 | 456 | A | ✅ 1164 |
| `src/bot_anthropic.rs` | 1524 | 1524 | 765 | 760 | A | ✅ 767 |

合计 38,697 行。其中 **内联测试 12,866 行（33%）**——这部分是「假重量」：
它不占生产逻辑，但照样把文件顶到超限线上。**先把它挪走，8 个文件直接
达标**。

**A 类 8 条已全部落地（2026-10-25）**：白名单 18 → 10 条，减债 8,394 行。
落地过程中新发现三处计划未预判的坑，见 §九。

**B 类 8 条 + C 类 1 条已落地（2026-10-25 续）**：白名单 10 → **1 条**
（只剩 `src/bot/registry.rs` 3827）。B/C 类的实际切法、落地偏差与新发现的
四个坑见 §十。

> **原计划清单漏排两条**：`src/bot/dispatch.rs`（B 类，1819）与
> `src/py/document.rs`（B 类，2456）**没有对应批次**——§三 的 12 批只覆盖了
> 16 个文件。这两条是实测基线里有、计划里没排的，执行时按同一手法补做
> （批次 1 / 批次 6c）。

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

| 批 | 内容 | 类别 | 触及文件 | 预计减债 | 风险 | 实际落地（commit） |
|---|---|---|---|---|---|---|
| 1 | `db/mod.rs` 测试外移 | A | 1 | ~2060 | 极低 | ✅ `2f909231` |
| 2 | `audit.rs` 测试外移 | A | 1 | ~1035 | 极低 | ✅ `0b3c13df` |
| 3 | `bot/mcp/manager.rs` 测试外移 | A | 1 | ~863 | 极低 | ✅ `bf66283b` |
| 4 | `apps/manage.rs` 测试外移 | A | 1 | ~919 | 低 | ✅ `9d60cf67` |
| 5 | `bot_anthropic.rs` + `bot_web.rs` + `bot_scheduler.rs` 测试外移 | A | 3 | ~1867 | 极低 | ✅ `97f6af2e` |
| 6 | `bot_skills/manage.rs` 测试外移 + 生产切分 | A+B | 1 | ~160+ | 低 | ✅ `7306a2d4` |
| 7 | `db/tasks.rs` 生产切分 | B | 1 | ~1500 | 中 | ✅ `50ea76c2` |
| 8 | `db/trace.rs` 生产切分 | B | 1 | ~1650 | 中 | ✅ `5f745f79` |
| 9 | `bot_fs.rs` 生产切分 | B | 1 | ~1880 | 中 | ✅ `b325f6dc` |
| 10 | `bot/tools.rs` + `bot/services.rs` 生产切分 | B | 2 | ~3000 | 中高 | ✅ `6e5974d0` / `eb9c3eeb`（拆成两个 commit） |
| 11 | `bot_chat.rs` + `bot/registry.rs` 生产切分 | B | 2 | ~3300 | 高 | 🔶 `bot_chat` ✅ `93658dba`；`registry` ⬜ 未开始 |
| 12 | `tests/llm_integration.rs` 拆分 | C | 1 | ~4650 | 中高 | ✅ `694870af` |
| 补 1 | `bot/dispatch.rs` 测试外移（**原清单漏排**） | B | 1 | ~810 | 极低 | ✅ `bfe0ab50` |
| 补 2 | `py/document.rs` 生产切分 + Python 脚本真搬（**原清单漏排**） | B | 1 | ~1360 | 中 | ✅ `8eca65b1` |

**批次 1–6 合计减债 ~8,400 行**，18 → 12 条（A 类 8 条全部清账）。
**补 1/补 2 + 批次 7–12 全部落地后**：白名单 **18 → 1 条**（只剩 `bot/registry.rs`）。

> **执行顺序与原计划不同**：原计划按「A 类 → B 类 → C 类」排，实际按
> **依赖序**排（先做耦合最松的，再啃耦合最紧的）——`llm_integration`（C 类）
> 提前到第 4 批做掉，`bot_chat`（原批次 11）提前到第 5 批。原因：C 类这条
> 是**独立 crate**，不与任何生产模块耦合，先做掉不影响后面的手感；而
> `bot_chat` / `tools` / `services` 三者共享 `TOOLS_TABLE` 与命令注册面，
> 放在一串里连做会把改动面叠在一起。
>
> **每文件一个 commit**（不合并批次）：批次 10 的两个文件、批次 11 的两个文件
> 都拆成了独立 commit，回退粒度按文件。

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

### 实际落地口径与本节描述的差异（A 类执行后修订）

原计划给批次 1–6 写的是「`tests/` 目录 + 门面 + 若干桶文件」三段式
（照 `bot_model_loop/tests/` 的样子）。实际落地全部改成了**两段式**：

```
src/db/mod.rs (416)          生产代码
src/db/tests.rs (1461)       原 mod tests 的块体，一个文件
src/db/ws_tests.rs (414)     原 mod ws_tests
```

理由：**两段式能让 `super` 语义完全不变**。抽出的文件仍是父模块的
**直接子模块**（Rust 2018：父文件 `src/a.rs` 的子模块在 `src/a/X.rs`），
所以 `use super::*` 仍解析为 `crate::db::*`、`super::lock_db_write`、
`super::tasks::CONFLICT_ERR_PREFIX` 全部原样有效——**纯搬移，零语义改动**。
三段式（桶文件再嵌一层）会把 `super` 指向桶的父级，`super::` 引用全断，
逐个改引用就成了「顺手重构」，违反纯搬移红线。

代价：一个桶文件可能仍偏大（`db/tests.rs` 1461 行，贴着 1500 上限）。
本批不切——它已在限内，测试代码不进生产路径，不构成「该拆了」的信号。
若将来要切，切点是主题而非层级（PRAGMA / legacy 拷贝 / 迁移 / 任务文件 /
bot_history / upsert 语义），且切完仍保持「直接子模块」形态。

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
| 1 | ✅ 完成 | `2f909231` | 销 `src/db/mod.rs` | 2468 → 416，测试外移 |
| 2 | ✅ 完成 | `0b3c13df` | 销 `src/audit.rs` | 1749 → 717；含冻结守卫口径修复（新增 `bot/test_files.rs` 共享模块） |
| 3 | ✅ 完成 | `bf66283b` | 销 `src/bot/mcp/manager.rs` | 2075 → 1216；另修 `bot/procs` 同源守卫缺陷 |
| 4 | ✅ 完成 | `9d60cf67` | 销 `src/apps/manage.rs` | 1884 → 966，测试外移 |
| 5 | ✅ 完成 | `97f6af2e` | 销 `bot_anthropic` / `bot_web` / `bot_scheduler` | 1524→767 / 2050→1400 / 1617→1164 |
| 6 | ✅ 完成 | `7306a2d4` | 销 `src/bot_skills/manage.rs` | 1664 → 625；生产切分（create.rs 337 行）+ 测试外移 |
| 7 | ✅ 完成 | `50ea76c2` | 销 `src/db/tasks.rs` | 2467 → 958，生产切分（`tasks_patch.rs` / `tasks_transfer.rs`）+ 测试外移 |
| 8 | ✅ 完成 | `5f745f79` | 销 `src/db/trace.rs` | 3040 → 908，生产切分（`trace_query.rs` / `trace_usage.rs` / `trace_session.rs`）+ 测试外移 |
| 9 | ✅ 完成 | `b325f6dc` | 销 `src/bot_fs.rs` | 2846 → 465（门面）+ `bot_fs/{edit,read,receipt}.rs`；`audit_log(` 冻结白名单同步补新路径 |
| 10a | ✅ 完成 | `6e5974d0` | 销 `src/bot/tools.rs` | 2949 → 88（门面）+ `bot/tools/{common,task,doc,service}.rs` + 9 个测试文件 |
| 10b | ✅ 完成 | `eb9c3eeb` | 销 `src/bot/services.rs` | 2391 → 1332（门面）+ `bot/services/{gate,logs,observe}.rs`（原计划只列了 `gate`，实际切 3 块） |
| 11a | ✅ 完成 | `93658dba` | 销 `src/bot_chat.rs` | 3028 → 419（门面）+ `bot_chat/{flow,exec,media,summary,report,input}.rs` + 4 个测试文件 |
| 11b | ⬜ 未开始 | | | `bot/registry.rs` 3827，172 处引用，最险——**需重新论证**（见 §十一） |
| 12 | ✅ 完成 | `694870af` | 销 `tests/llm_integration.rs` | 4645 → 目录形态 6 文件（`core` / `sse` / `e2e` / `guard` / `anthropic` / `main`） |
| 补 1 | ✅ 完成 | `bfe0ab50` | 销 `src/bot/dispatch.rs` | 1819 → 1009，内联测试外移到 `bot/dispatch/*_tests.rs`（原 12 批清单漏排） |
| 补 2 | ✅ 完成 | `8eca65b1` | 销 `src/py/document.rs` | 2456 → 1097；8 段内嵌 Python **真搬成 `py/scripts/*.py`**（`include_str!` 载入，正文逐字节一致）+ 7 个测试文件 |

**A 类小结（2026-10-25）**：白名单 18 → 10 条，减债 8,394 行；
Rust 2191 例 + 前端 1065 例全绿；tests-audit 41 例全绿。
源文件数 186 → 206（新增 20 个测试子文件），总行数基本不变（12.6 万）。

**B/C 类小结（2026-10-25 续）**：白名单 10 → **1 条**，只剩 `bot/registry.rs`。
Rust 2191 passed / 11 skipped、前端 100 files / 1065 tests、tests-audit 41 passed、
`audit_no_eval.py` 通过、`cargo fmt --check` 与 `cargo clippy --all-targets -D warnings`
干净、工单号防线通过。源文件数 206 → **262**，总行数 12.6 → **12.5 万**
（减债主要来自把内嵌测试拆成独立文件后不再有重复的 import 噪音）。
每批都是**纯搬移**：多重集比对确认「原文件行零丢失，新增行只有 `use` 增删 /
`mod` 声明 / 可见性修饰 / 模块文档头」。

---

## 九、A 类落地时新发现的三个坑（计划未预判）

这一节是执行阶段的实际产出——三条都不是原计划列的，但都差点造成静默
失效或误报。

### 坑一：位置启发式在两个门禁里都是错的

原计划以为「按首个 `#[cfg(test)]` 之前切生产段」够用。实际它有两个独立缺陷：

1. **扫不到**：`bot_fs.rs` 的首个 `#[cfg(test)]` 在第 75 行，于是它 2000 多行
   生产代码（含 20 处 `audit_log(` 调用）整段落在「测试区」——扫描范围
   取决于代码摆放位置，而不是代码是不是测试。
2. **切太早**：本次在 `bot_skills/manage.rs` 文件头加了一行
   `#[cfg(test)] pub(crate) use create::create_skill_at;`（测试专用 re-export），
   于是「首个 `#[cfg(test)]」从第 942 行变成第 15 行，源码文本守卫把后面
   大段生产代码整段切掉 → 两个守卫当场 fail。

**处置**：两个扫源码的门禁（`audit.rs` 的冻结守卫、`bot/procs` 的控制台
静默守卫）口径统一到新模块 `bot/test_files.rs`：
- 测试专用文件按 `#[cfg(test)] mod X;` **声明**识别（Rust 的模块文件规则
  唯一：父文件 `src/a.rs` 与 `src/a/mod.rs` 的子模块都在 `src/a/X.rs`）——
  不靠猜文件名（`bot/eval.rs` / `audit/freeform_freeze.rs` 是测试专用但
  名字里没 "test"；`bot/registry.rs` 名字像测试文件却是生产代码）。
- 生产切片 = 全文剔除全部 `#[cfg(test)]` 块，与位置无关。

**顺带修掉一个存量盲区**：`bot_fs.rs` 那 20 处 `audit_log(` 调用此前
从未被冻结守卫扫到（靠位置侥幸躲过）。新口径下它们进扫描范围，
`bot_fs.rs` 本就在白名单 → 仍 pass，但盲区没了。

### 坑二：拆分后 `audit_log(` 的文件路径变了，冻结白名单要跟

`bot_skills/manage.rs` 的 2 处 `audit_log(` 搬到
`bot_skills/manage/create.rs`——**同一批存量调用，只是路径变了**。
冻结白名单按路径登记，路径不变就不认 → 守卫当场 fail。

这不是「新增违规」，是「存量违规换了地址」。处置：白名单加新路径
（`bot_skills/manage/create.rs`），原路径保留。

**推论（对 B 类同样成立）**：B 类每次拆文件都可能触发这条——
`bot/tools.rs`（8 处）、`bot_fs.rs`（20 处）、`bot_chat.rs`（6 处）、
`bot_scheduler.rs`（18 处）、`bot_web.rs`（2 处）、
`apps/manage.rs`（4 处）、`py/document.rs`（4 处）都在白名单里，
拆一次就要跟一次。**要么同 commit 改白名单，要么先把该文件的
`audit_log(` 迁成 `audit_event!`（一劳永逸）。**

### 坑三：朴素括号计数在 Rust 源码上不可靠

拆分脚本最初用「数 `{`/`}` 找配对」定位 `mod X { ... }` 的结束行。
在 `bot_web.rs` 上翻车——测试块里有**跨行原始字符串** `r#"..."#`，
按行扫时字符串里的 `{`/`}` 被当代码括号，块边界算错。

**处置**：改用跳过字符串 / 字符字面量 / 行注释 / 块注释的语法感知定位，
且原始字符串按整段文本（可跨行）匹配闭合标记。
`bot_anthropic.rs` 同类问题（文件里字符串含未配平花括号）。

**这条对 B 类同样成立**——`bot/registry.rs`（63 个 schema 字面量）、
`bot/tools.rs`、`py/document.rs`（大段内嵌 Python 脚本）都是字符串密集区。

---

## 十、B/C 类落地时的实际切法与新发现的坑（2026-10-25 续）

§九 的三个坑是 A 类（测试外移）撞出来的。B/C 类（生产切分）撞出另外四个，
都是「切分手法」层面的，不是「文件内容」层面的。

### 实际切法与 §三 计划的出入

| 文件 | 计划 | 实际 | 差异原因 |
|---|---|---|---|
| `bot/dispatch.rs` | 未排 | 生产不动 + 5 个 `*_tests.rs` | 内联测试只 294 行，纯外移就够，无需生产切分 |
| `db/tasks.rs` | ~1050 | 958（+ `tasks_patch.rs` / `tasks_transfer.rs`） | 计划的「~1050」是按「切一块」估的，实际两块都切才落进 1500 |
| `db/trace.rs` | ~1650 | 908（+ `trace_query.rs` / `trace_usage.rs` / `trace_session.rs`） | 同上，切三块 |
| `bot_fs.rs` | ~1500（7 文件） | 465（门面）+ 3 生产 + 4 测试 | 计划低估了测试块占比 |
| `bot/tools.rs` | ~1450 | 88（门面）+ 4 生产 + 9 测试 | 同上 |
| `bot/services.rs` | ~1200（门面 + `gate`） | 1332（门面）+ `gate` / `logs` / `observe` | 计划只列了 `gate` 一块；实测资源观测面与日志面各自独立，切一块放不下 |
| `bot_chat.rs` | ~1400 | 419（门面）+ 6 生产 + 4 测试 | 同上 |
| `py/document.rs` | 未排 | 1097 + `py/scripts/*.py`（8 个真 `.py` 文件） | 见下 |
| `tests/llm_integration.rs` | 3 块 | 6 块 | **独立 crate**：`tests/` 下每个 `.rs` 是独立 target，子目录里的文件不会被自动编译 → **只能平铺** |
| `bot/registry.rs` | 4 块 | 见 §十一 | 原计划的切法会打断 3 个接线守卫 |

**`py/document.rs` 的额外决策**：它内嵌 8 段 Python（`const` + `r#"..."#`），
搬成 Rust 子模块里的 `const` 后**行数照旧**（一行超长 + 编辑器无高亮 +
Rust 工具链管不着）。改成**真 `.py` 文件**（`src-tauri/src/py/scripts/*.py`），
Rust 侧 `include_str!` 载入——正文逐字节不变，编辑器高亮与语法检查恢复。
这是 §三 之外新增的一种切法类别：**「非 Rust 载荷」单独成文件**。

### 坑四：`pub(crate) use` 与私有 `use` 对「子模块 glob 取用」的口径不同

门面把子模块的项转出来给仓库其他地方用，有四种写法，**选错会被
`unused_imports` 打回**：

| 用途 | 写法 | 判定口径 |
|---|---|---|
| 仓外可达（公开 API） | `pub use` | **永不被判 unused**——re-export 本身就是出口 |
| 仓内可达（crate 内有引用） | `pub(crate) use` | 仓外**无人引用**时报 unused（`bot/tools.rs` 的 `TaskView` / `resolve_task_key` 即因此改成 `#[cfg(test)] use`） |
| 仅门面自身 + 子模块经 `use super::*` 取用 | 私有 `use` | 子模块的 glob 取用**计入**「已使用」，不报 unused（`bot_chat` / `bot/tools` 已实证） |
| 仅测试用 | `#[cfg(test)] use` | 同私有 `use`，且非测试构建下整个绑定消失 |

**推论**：门面 re-export 清单要按**「仓外有没有人用」**分档，不是按
「原来是不是 pub」。原来 `pub(crate)` 但仓外零引用的项，转进子模块后
应降成 `pub(super)`（原可见域 = 该模块）+ 门面私有 `use`。
反过来，原来 `pub` 的项必须**逐项 `pub use` 转出**，否则公开路径断掉。

### 坑五：源码文本守卫随搬移失效，必须改指新家

`services/tests.rs` 的 `patrol_wires_canary_before_judging` 用
`include_str!("../services.rs")` 读**生产源码文本**，断言「金丝雀判定排在
采样状态记录之前」。`enforce_listening_gate` 搬到 `gate.rs` 后守卫读的
是门面——门面里没有那个函数 → 守卫 fail。

**一般规则**：凡是 `include_str!` / `read_to_string` 读源码文本的守卫，
搬移后必须改指新家；且**不能改成扫整个目录**——守卫里的签名字面量会扫到
测试文件自身而自命中（`bot/tools.rs` 的 `sync_syscall_off_runtime_tests` 踩过，
改法是 `include_str!` 显式列出生产文件）。

### 坑六：`prod_slice` 遇 `#[cfg(test)]` 后的「找 `mod `」是纯文本定位

`bot/test_files.rs::prod_slice` 遇到 `#[cfg(test)]` 后，会在**后续文本**里
找第一个 `mod `，再从那里扫到配对的 `}` 整段丢弃。这个定位是**纯文本**的——
注释或字符串里出现 `mod ` 就会误指，误指后从那里到下一个 `}` 的**生产代码
会被整段丢弃**。

**处置**：门面的 `#[cfg(test)] use` 块**必须放在文件末尾**（生产代码之后、
`#[cfg(test)] mod X;` 之前），且注释里别出现 `mod ` 字样。
（`bot/services.rs` 门面末尾的 `#[cfg(test)] use gate::{...}` 块就踩在这条线上。）

### 坑七：枚举仓外引用时，`use crate::X::{a, b, c}` 的组形式必须一并 grep

只扫 `crate::X::Y` 形式会漏掉**整组 import**——
`pub(crate) use crate::bot_fs::{build_unified_diff, sha256_hex, READ_MAX_BYTES};`
这一条在 `bot_fs` 拆分时就漏了，直到编译器报 unresolved 才补上。

**处置**：枚举引用时同时 grep `use crate::X::` 与 `use crate::X::{` 两种形态。

### 坑八（门禁侧）：拆完必须同步三处文档 + 白名单，缺一条就红

`audit_module_map.py` 强校验三处：**模块树条目（`(NNN)` 行数标注须精确）**、
**§6.4 源文件清单**、**开头散文「共 N 个文件、约 M 万行」**。
文件数与总行数都要**重新算**——每拆一个文件，文件数 +1，总行数基本不变
（拆测试块时 import 噪音反而减少）。`AGENTS.md` 与 `docs/testing.md` 里的
「白名单 N 条」也要跟着改（它们是散文里的数字，脚本不校验，但会误导人）。

---

## 十一、`bot/registry.rs` 的重新论证（2026-10-25）

§三 批次 11 把 registry.rs 标成「最险」，并说「需重新论证」。重新论证的
结论：**该拆，但切法要改**——原计划那套切法会打断三个「接线守卫」。

### 重新论证：三个原判断的实测修正

| 原判断（计划 / 早前口径） | 实测 | 影响 |
|---|---|---|
| 「全仓 172 处 `crate::bot::registry::` 引用」 | 172 是**引用次数**；外部引用的**项**只有 **27 个**，分布在 48 个文件（`ToolResult` 125 次、`ToolStatus` 25 次，其余 25 项各 1–8 次） | 门面逐项 re-export **27 项 + 63 个 `SCHEMA_*`**（后者公开可见，零仓外引用但属公开 API，得转出保住路径）——量级可控，不是「172 处都要改」 |
| 「63 个同质 `call_*`」是痛点 | 它们是**一行转发**（`call_query_tasks` → `crate::bot::tools::tool_query_tasks`），423 行合一块 | 不是痛点，合到 `handlers.rs` 即可，不必拆散 |
| 「痛点是横向耦合」 | 真痛点是**6 个源码文本接线守卫** | 决定切法，见下 |

### 真痛点：6 个「接线守卫」读的是 `bot/registry.rs` 的**文本**

| 守卫 | 位置 | 断言 |
|---|---|---|
| `ui_tools_are_wired_into_registry` | `ui/mod.rs` | 6 个 UI 工具名出现在 `name: "X"` |
| `ui_tools_are_wired_into_registry` | `bot_ui.rs` | 同上 |
| `manage_file_is_wired_into_registry` | `bot_manage_file.rs` | `name: "manage_file"` **且** `SCHEMA_MANAGE_FILE` |
| `copy_file_to_clipboard_is_wired_into_registry` | `bot_desktop.rs` | `name: "copy_file_to_clipboard"` **且** `SCHEMA_COPY_FILE_TO_CLIPBOARD` |
| `reminder_tools_are_wired_into_registry` | `bot_reminders.rs` | 3 个提醒工具名 **且** `SCHEMA_SCHEDULE_REMINDER` |
| `tools_table_has_no_cfg_gated_entries` | `bot/registry.rs` 内 | `TOOLS_TABLE` 体内不许有 `#[cfg(` |

后三个断言 **`name:` 与 `SCHEMA_` 在同一个文件文本里**。若按原计划把
`SCHEMA_*`（`schema.rs`）与 `TOOLS_TABLE`（`table.rs`）拆开，这三个守卫
就得改成读两个文件——守卫语义从「接进工具表」退化成「两处都有」，
**反而更弱**（原来「同一文件」蕴含了「同一份接线」，改完就没有这个保证了）。

**结论：`SCHEMA_*` 与 `TOOLS_TABLE` 必须留在同一个文件。**

### 切法（与原计划不同）

```
src/bot/registry.rs (~290)                类型（ToolCtx/ToolStatus/ToolResult/ToolDef）
                                          + 门面逐项 re-export（87 项：63 个 SCHEMA_* / 11 个表侧 / 13 个判据侧）
src/bot/registry/schemas.rs (~1160)       63 个 SCHEMA_* 常量 + TOOLS_INDEX / TOOLS_TABLE /
                                          assemble_tools_json / tools_json* / profile_* / mutating_tools
src/bot/registry/handlers.rs (~430)       63 个 call_* 转发（私有一行转发）
src/bot/registry/claims.rs (~760)         声明判据（claims_mutation / claims_execution /
                                          数字抽取 / 路径抽取 / mentioned_tool_names）
src/bot/registry/mentioned_tool_names_tests.rs   原 1541–1621 内联块
src/bot/registry/claimed_executed_tools_tests.rs 原 1622–1706 内联块
src/bot/registry/result_claim_tests.rs           原 2689–2788 内联块
src/bot/registry/registry_tests.rs               原 2789–3827 内联块
```

**跨模块接线**（门面是唯一 hub，子模块经 `use super::*` 取用）：

```
schemas.rs  ← SCHEMA_*（同文件）+ call_*（来自 handlers.rs）
handlers.rs ← 无（纯转发，只依赖 crate::bot::tools）
claims.rs   ← TOOLS_TABLE（来自 schemas.rs）+ TOOL_ALIASES（同文件）
```

**外部改动面**：只有 6 个守卫的 `include_str!` 路径（各改 1 行，指向
`registry/schemas.rs`）——**没有一处外部调用点需要改**（27 项路径由门面
re-export 保住）。

**不动的三样**（§七 不做清单）：
- `TOOLS_TABLE` 的**条目顺序**——整块搬移，顺序天然不变；
- 63 个 schema 的**字面量**——逐字节搬移，正文不改；
- `BUILTIN_TOOL_COUNT` 手写常量与表长度的关系——表整体搬移，条数不变。

**风险与兜底**：
1. 门面 re-export 漏一项 → 编译器报 unresolved（**兜底可靠**）；
2. 守卫漏改路径 → 守卫当场红（**兜底可靠**）；
3. 可见性收窄 → 编译器兜住（注意 `pub(super)` 在子模块里指向**该子模块的
   父级**，不是原来的 `crate::bot::registry`——搬运时要逐项按「原可见域 =
   `crate::bot::registry`」翻译，别照抄 `pub(super)`）。
