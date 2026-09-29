# Batch B 重审 + 修复报告（2026-08-19）

范围：`src-tauri/src/db.rs`（1470 行）、`src-tauri/src/migration.rs`（1483 行）、`src-tauri/src/api.rs`（105 行）、`src-tauri/src/lib.rs`（408 行，db/migration 命令入口）。基线 commit：`e84a5e6`。

## 阶段 1：审计结果

### 已知 B1-B4 + P2 状态复核

**重要更正：B1-B4 在当前代码库中已全部修复**（任务背景假设未修，实际已落地）：

| ID | 状态 | 证据 |
|---|---|---|
| P0-1 | ✅ 已修 | `11e9ec9`，bot_history_save / bot_session_delete 事务包裹 + 回归测试在 `db.rs:1169-1300` |
| B1 | ✅ 已修 | `9a52272`，migration_journal 表 + pending/committed/cleared + 启动 replay |
| B2 | ✅ 已修 | `ae17174`，`upsert_tasks` ON CONFLICT ... WHERE updated_at 守卫（`db.rs:694-698`）+ 5 场景测试 |
| B3 | ✅ 已修 | `02a3ecc` + `33910c9`，db_load/db_upsert/db_delete/db_merge/tasks_export/tasks_import/bot_history_*/bot_session_delete/migration_run 全部 async + spawn_blocking |
| B4 | ✅ 已修 | `84d007a`，`r.get::<_, Option<i64>>(0)` + flatten + unwrap_or(0)（`db.rs:1034-1042, 1093-1101`）+ NULL 兼容测试 |
| P2-4 | ⚠️ 仍在 | 便携首启 legacy 拷贝：`wal_checkpoint` 错误被吞 + 不拷 `-wal`（`db.rs:86-96`） |
| P2-5 | ⚠️ 仍在 | `upsert_workspace`/`delete_workspace` 循环 execute 无事务（`db.rs:411-450`） |
| P2-6 | ⚠️ 仍在 | `conflict_free_name` 检查后 `fs::rename` Unix 上覆盖已存在目标（`migration.rs:449-465` + `move_entry`） |
| P2-7 | ⚠️ 仍在 | `migrate_data_json` 触发条件仅 `count==0` + 便携模式只查 app_data_dir（`db.rs:834-854`） |
| P2-8 | ⚠️ 仍在 | `save_rules` 直接 `fs::write` 覆盖（`migration.rs:386-391`） |

### 新发现列表（按严重度排序）

| ID | 位置 | 角度 | 严重度 | 一句话问题 |
|---|---|---|---|---|
| NEW-B-1 | migration.rs 阶段二解绑分支 + journal_replay_pending | 错误恢复 / 回滚语义 | P1 | journal 只在启动时 replay，轮询中「源已消失」直接解绑不查 journal → 附件链接整会话期丢失；且重启 replay 无条件回写 file_path，可覆盖用户期间重绑的新附件 |
| NEW-B-2 | migration.rs:53-119（journal_pending/committed/cleared） | 并发 | P2 | journal 三次写各开独立 open_db 连接且不持 DB_WRITE_LOCK，与主窗口写并发只靠 2s busy_timeout 兜底；每迁一个文件付 3 次全套 schema 检查开销 |
| NEW-B-3 | db.rs workspace_*/bot_session_create/rename/bot_sessions_load + migration.rs migration_log_read 等 | 并发 / 主线程阻塞 | P2 | B3 残留 sync 命令仍跑主线程；migration_log_read 主线程读最大 5MB 日志。数据量小，风险低 |
| NEW-B-4 | db.rs:270-276（RESET_ONCE） | 错误恢复 | P2 | 清残留 bot_assigned 的 UPDATE 在写锁外执行且 `let _` 吞错 + Once 不可重试：首开遇库忙则残留 🤖 标志要到下次重启才清 |
| NEW-B-5 | migration.rs move_entry 跨卷回退 | 错误恢复 | P2 | 跨卷 copy 成功但 remove 源持续失败（Windows 文件占用）时，每轮轮询 conflict_free_name 生成新名再 copy → 归档目录重复副本无限累积 |
| NEW-B-6 | db.rs:1058-1069（tasks_export） | 资源生命周期 | P2 | 导出 `fs::write` 非原子（与 P2-8 同类）：导出中崩溃留半截 JSON，用户当备份用有风险 |

### 新发现详情

#### NEW-B-1（migration.rs 阶段二 + journal_replay_pending）— 已修 `d097e46`
- **症状**：move_entry 成功但 db_upsert 瞬时失败后，10 分钟下一轮轮询把任务 file_path 置 None（「解除绑定」），用户在本次会话内看到附件丢失；且 pending journal 要等**下次重启**才 replay。更坏：若用户在间隔期给任务重新绑定了别的文件，重启 replay 的 `recover_move_db`/`recover_delete_db` 无条件回写，**把新绑定覆盖掉**。
- **根因**：B1 修复只把 replay 挂在 `spawn_polling` 启动段（`migration.rs:779`），`run_migration_inner` 的「源已消失」分支不查 migration_journal；recover_* 无幂等键（不校验任务当前 file_path 是否仍等于 journal 记录的 src）。
- **修复策略**：
  1. 新增 `journal_find_pending(_inner)`：按 (task_id, src) 查最新 pending；
  2. 新增纯函数 `decide_src_missing`：move pending + dst 存在 → `RepairMove`（就地重新绑定 dst 并立即提交 journal）；其余 → `Unbind`（解绑落盘成功后提交对应 pending journal，关闭对账环路）；
  3. `recover_move_db`/`recover_delete_db` 增加 `expected_src` 参数 + `file_path_untouched` 守卫：任务 file_path 已不指向 src（用户重绑/已解绑）→ 返回 Ok(false)，replay 跳过并 clear journal，不覆盖。
- **修复 commit**：`d097e46`
- **新增测试**（4 个，migration::tests）：
  - `journal_find_pending_finds_matching_entry`（happy path）
  - `journal_find_pending_ignores_non_pending_and_other_keys`（committed/cleared/他 task/他 src 不命中 + 多条取最新）
  - `decide_src_missing_all_branches`（5 分支全覆盖）
  - `file_path_untouched_guard`（指向 src 才允许修复；重绑/已解绑禁止覆盖）

#### NEW-B-2 ~ NEW-B-6
见阶段 3 表格（P2，按规则不修）。

### 各角度扫查结论（无新问题的角度）

- **SQL 注入/参数化**：全库参数化；`format!` 拼 SQL 仅 `delete_tasks` 的 `?` 占位符与 `ALTER TABLE`/`load_external` 的静态列名，无注入面。
- **批量写一致性**：db_upsert/db_merge/tasks_import/migrate_data_json 均有事务；bot_history_save/bot_session_delete 已修（P0-1）。
- **lib.rs 入口**：invoke_handler 中 db/migration 命令全部 async spawn_blocking，无 Batch A 风格遗漏；快捷键 handler 不触 DB。
- **api.rs（数据层）**：TauriStore 的 `block_on` 桥接在 per-request std::thread 上（非 tokio 线程），无死锁；api_handlers 写路径（create/update/delete）均 `updated_at = Some(now)`，与 B2 守卫兼容，**不存在「API 更新被守卫静默丢弃」问题**（已核实排除）。
- **WAL/checkpoint**：每命令独立连接，最后连接关闭时 SQLite 自动 checkpoint，便携退出路径安全；legacy 拷贝问题即旧 P2-4。

## 阶段 2：实际修复汇总

| ID | 严重度 | commit | 改动 | verify |
|---|---|---|---|---|
| NEW-B-1 | P1 | `d097e46` | migration.rs +344/-36：2 个 journal 查询/决策助手 + recover_* 防覆盖守卫 + 两处解绑分支对账 + 4 个单测 | build 干净；migration 35 测试全过 |

## 阶段 3：未修的（留到下个 batch）

| ID | 严重度 | 一句话问题 | 建议修复策略 | 估计工期 |
|---|---|---|---|---|
| NEW-B-2 | P2 | journal 写不持 DB_WRITE_LOCK + 每文件 3 次 open_db | run_migration_inner 全程复用一条连接（journal 操作传 &conn），写路径统一走写锁 | 0.5 工日 |
| NEW-B-3 | P2 | B3 残留 sync 命令（workspace_*、bot_session_create/rename、migration_log_read 等） | 比照 33910c9 改 async + spawn_blocking | 0.5 工日 |
| NEW-B-4 | P2 | RESET_ONCE 清 bot_assigned 吞错不可重试 | 失败时不清 Once（或改每次启动常规 SQL 重试），并纳入写锁 | 0.2 工日 |
| NEW-B-5 | P2 | 跨卷 remove 持续失败 → 归档副本无限累积 | copy 成功后 remove 失败时记录「dst 已拷贝」状态（journal 扩展或日志指纹），下轮跳过重复 copy | 0.5 工日 |
| NEW-B-6 | P2 | tasks_export 非原子写 | 写 tmp + rename（与 P2-8 同一修法一起做） | 0.2 工日 |

## 总体 verify

- `cargo build`：✅ 干净，仅 2 个 pre-existing warning（`use crate::db::Task` unused import + JournalEntry state/created_at dead_code，已用 git stash 对比确认为基线原有）
- `cargo test --lib`：✅ 232 passed（基线 228 → **净新增 4**）；2 failed 为 pre-existing（`scan_all_skills_in_debug_dir_parse_correctly` + `smoke_all_real_skills_run_dsl_loop_with_mock_executor`，C 路径 DSL 迁移遗留，不在范围），与基线完全一致
- 净新增测试数：**4**（均在 migration::tests）

## Commits

- `d097e46` fix(db): NEW-B-1 源消失时对账 migration_journal 就地修复 + replay 防覆盖（P1, src-tauri/src/migration.rs）

报告路径：`docs/kimi-audit/BATCH-B-REAUDIT-2026-08-19.md`
