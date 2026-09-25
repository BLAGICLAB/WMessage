# Batch Spec: FRB-01-FAILCLOSED

## 目的

C5-DB-05 (4 条 silent mutex poisoning 替换为默认值 → 真传播) + C5-DB-01b (2 条 workspace links 序列化失败 → 返 Err 阻止数据覆写) + C5-BT-01b (2 条 DB load 失败 → 返 Err 不吞为 "无任务")。本批走 fail-closed 路径，统一返 Err 给调用方。

## 人类可读摘要

- family: error-not-propagated（动作形态属真传播；原 failure-recovery-default-value 命名漂移——本批以"返 Err"消除 silent-discard，不写默认值）
- 覆盖 findings: 8（DB-05:4 + DB-01b:2 + BT-01b:2）
- 预估 diff: 3 files / +30/-15 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 8

## 红线

- family 一致性：本批只含 error-not-propagated（family 漂移明确记录在 PHASE2-TRIAGE.md §1）
- 0 FP / 0 family 异质
- 不写"空 Vec" / `unwrap_or_default()` / `into_inner()` 吞真错误——全部改 `?` / `match Err(e) => return Err(...)` 显式传播
- 签名变化：DB-05 函数签名不变（仍 `CommandResult<_>`），只改 body 内部错误处理；DB-01b / BT-01b 函数签名改 `-> Result<_, _>`

## Stop 条件（触发即停，报 reviewer）

- compile_failure
- architecture_blocker
- family_heterogeneity
- new_high_different_root
- gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FRB-01-FAILCLOSED",
  "family": "error-not-propagated",
  "expected_files": [
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workspace.rs",
    "src-tauri/src/bot/tools.rs"
  ],
  "max_lines_added": 35,
  "max_lines_removed": 20,
  "findings": [
    {"id": "FRB-01.1", "file": "src-tauri/src/db/tasks.rs", "line": 471, "fix": "db_upsert_for 内 lock_db_write().unwrap_or_else(|e| e.into_inner()) 改 lock_db_write().map_err(|e| CommandError::from(format!(\"[mutex_poisoned] db::DB_WRITE_LOCK: {e:?}\")))?; —— 与 lib.rs:758 mutex_poisoned 约定一致"},
    {"id": "FRB-01.2", "file": "src-tauri/src/db/tasks.rs", "line": 519, "fix": "tasks_import 内 lock_db_write() 同样加 ? + mutex_poisoned log（与 .1 一致）"},
    {"id": "FRB-01.3", "file": "src-tauri/src/db/tasks.rs", "line": 450, "fix": "db_delete 内 lock_db_write() 同样加 ? + mutex_poisoned log"},
    {"id": "FRB-01.4", "file": "src-tauri/src/db/workspace.rs", "line": 129, "fix": "serde_json::to_string(&it.links).unwrap_or_else(|_| \"[]\".into()) 改 ? 传播 Err；函数 upsert_workspace_unchecked 改返 Result<_, String>"},
    {"id": "FRB-01.5", "file": "src-tauri/src/db/workspace.rs", "line": 254, "fix": "workspace_import 路径同 .4 改 ? 传播"},
    {"id": "FRB-01.6", "file": "src-tauri/src/db/workspace.rs", "line": 90, "fix": "validate_link_kinds 已返 Err — OK 不动"},
    {"id": "FRB-01.7", "file": "src-tauri/src/bot/tools.rs", "line": 147, "fix": "active_tasks db_load().await.unwrap_or_default() 改 ? 传播 Err；active_tasks 改返 CommandResult<Vec<Task>>"},
    {"id": "FRB-01.8", "file": "src-tauri/src/bot/tools.rs", "line": 160, "fix": "tool_list_tasks 接 active_tasks Err 传 ToolResult (含 err 字段)；tool_complete_task / tool_delete_task 内 db 操作同样 ? 传播"}
  ],
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 8
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```

## 签名核（执行前已验）

`db/tasks.rs:441` db_upsert_for（FRB-01.1 / .3 触发点）：`pub async fn db_upsert_for<R: tauri::Runtime>(...) -> CommandResult<()>`
`db/tasks.rs:464` db_delete（FRB-01.3）：pub async fn db_delete(...) -> CommandResult<()>
`db/tasks.rs:510` tasks_import（FRB-01.2）：pub async fn tasks_import(...) -> CommandResult<usize>
`db/workspace.rs:91` upsert_workspace（FRB-01.4 / .6）：pub fn upsert_workspace(...) -> Result<(), CommandError>
`db/workspace.rs:156` workspace_load（FRB-01.4 关联）：pub async fn workspace_load(...) -> CommandResult<Vec<WorkspaceItem>>
`bot/tools.rs:147` active_tasks（FRB-01.7）：async fn active_tasks(app: &AppHandle) -> Vec<crate::db::Task>
`bot/tools.rs:160` tool_list_tasks（FRB-01.8）：pub(crate) async fn tool_list_tasks(...) -> crate::bot::registry::ToolResult
`bot/tools.rs:438` tool_complete_task（FRB-01.8）：pub(crate) async fn tool_complete_task(...)
`bot/tools.rs:472` tool_delete_task（FRB-01.8）：pub(crate) async fn tool_delete_task(...)

## 调用点影响清单（5 个以内）

DB-01b / workspace 改返 Err 链：
1. `src-tauri/src/lib.rs:435` workspace_load 调用方（lib.rs 初始化路径）— 需验证已能处理 Result
2. `src-tauri/src/db/mod.rs:1297` upsert → load 回读（同 workspace_upsert 命令体）— 需验证已能处理 Result

BT-01b / tools 改返 Err 链：
3. `src-tauri/src/bot/registry.rs:252/264/270` Box::pin(async move { tool_* }) — 已 Result 链，链上接 Err
4. `src-tauri/src/bot/dispatch.rs:401` tool_complete_task / tool_delete_task 复用 completed_at / deleted_at — 需验证 Err 透传

总 4 路径（含引用 2 处），未超 5 阈值。

## 提交信息骨架

```
fix(db,bot): FRB-01-FAILCLOSED — DB-05/01b/BT-01b silent-discard → 返 Err 真传播（error-not-propagated family）

【family】error-not-propagated（原 failure-recovery-default-value naming 漂移，本批以"返 Err"消除 silent-discard）
【family drift 记录】PHASE2-TRIAGE.md §1 family 命名不修改；本批实际动作属 error-not-propagated，登记 family_heterogeneity 处置

【实修 8 处】（3 文件）
- db/tasks.rs（FRB-01.1-3）：db_upsert_for / db_delete / tasks_import 3 处 lock_db_write() 改 ? + mutex_poisoned eprintln log（与 lib.rs:758 约定一致）
- db/workspace.rs（FRB-01.4-5）：2 处 serde_json::to_string(&it.links) 改 ? 传播；upsert_workspace_unchecked 改返 Result
- bot/tools.rs（FRB-01.7-8）：active_tasks 改返 CommandResult<Vec<Task>>；tool_list_tasks / tool_complete_task / tool_delete_task 内部 db 操作 ? 传播

【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1：exit ? / ? comments / ?s（期望 ≤ 8）
【family】error-not-propagated（成员 +8）
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/FRB-01-FAILCLOSED.spec.md python3 scripts/batch-verify.py docs/batches/FRB-01-FAILCLOSED.spec.md
```