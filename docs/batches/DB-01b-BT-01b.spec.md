# Batch Spec: DB-01b-BT-01b

## 目的

C5-DB-01b (workspace links 序列化失败 silent-discard → 覆写用户数据) + C5-BT-01b (DB load 失败 silent-discard → "无任务"假象)。两条都走 fail-closed（返 Err 真传播），family = error-not-propagated（写死）。

C5-DB-05 (silent mutex poisoning) 拆出独立批，不属本批范围（涉及"改不改 C3-1 约定"独立产品决策）。

## 人类可读摘要

- family: error-not-propagated
- 覆盖 findings: 4（DB-01b:2 + BT-01b:2）
- 预估 diff: 2 files / +15/-10 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 红线

- family 一致性：本批只含 error-not-propagated（family 字段写死，不写"漂移"叙事）
- 0 FP / 0 family 异质
- 不写"空 Vec" / `unwrap_or_default()` / `unwrap_or_else(|_| "[]".into())` 等替换默认值的 silent 路径
- `upsert_workspace` 已 `Result<(), CommandError>` 不动签名；`upsert_workspace_unchecked` 内部 `?` 即可

## Stop 条件（触发即停，报 reviewer）

- compile_failure
- architecture_blocker
- family_heterogeneity
- new_high_different_root
- gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "DB-01b-BT-01b",
  "family": "error-not-propagated",
  "expected_files": [
    "src-tauri/src/db/workspace.rs",
    "src-tauri/src/bot/tools.rs"
  ],
  "max_lines_added": 30,
  "max_lines_removed": 20,
  "findings": [
    {
      "id": "DB-01b.1",
      "file": "src-tauri/src/db/workspace.rs",
      "line": 129,
      "fix": "serde_json::to_string(&it.links).unwrap_or_else(|_| \"[]\".into()) 改 ? 传播（函数 upsert_workspace_unchecked 已 Result<(), String>，Err 向上冒泡到 upsert_workspace CommandError）"
    },
    {
      "id": "DB-01b.2",
      "file": "src-tauri/src/db/workspace.rs",
      "line": 254,
      "fix": "DB-01b.1 同结构改 ? 传播"
    },
    {
      "id": "BT-01b.1",
      "file": "src-tauri/src/bot/tools.rs",
      "line": 147,
      "fix": "active_tasks 签名 Vec<Task> -> Result<Vec<Task>, String>；body crate::db::db_load(app.clone()).await.unwrap_or_default() 改 ? 传播"
    },
    {
      "id": "BT-01b.2a",
      "file": "src-tauri/src/bot/tools.rs",
      "line": 160,
      "fix": "tool_list_tasks:3 `let tasks = active_tasks(app).await;` 改 `match active_tasks(app).await { Ok(t) => 流程不变, Err(e) => return ToolResult::ok(text=format!(\"查询任务失败：{e}\"), refs=vec![], status=ToolStatus::Fail) }`"
    },
    {
      "id": "BT-01b.2b",
      "file": "src-tauri/src/bot/tools.rs",
      "line": 516,
      "fix": "tool_complete_task:516 active_tasks 调用改 match：Ok(t) => 流程不变, Err(e) => return ToolResult::ok(text=format!(\"完成任务失败：{e}\"), refs=vec![], status=ToolStatus::Fail)"
    },
    {
      "id": "BT-01b.2c",
      "file": "src-tauri/src/bot/tools.rs",
      "line": 531,
      "fix": "tool_delete_task:531 active_tasks 调用改 match：Ok(t) => 流程不变, Err(e) => return ToolResult::ok(text=format!(\"删除任务失败：{e}\"), refs=vec![], status=ToolStatus::Fail)"
    }
  ],
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 6
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

`src-tauri/src/db/workspace.rs:97` upsert_workspace：pub fn → Result<(), CommandError>（已 Result）
`src-tauri/src/db/workspace.rs:105` upsert_workspace_unchecked：fn（私有）→ Result<(), String>（已 Result，DB-01b.1 改 `?` 让 Err 向上冒泡）

`src-tauri/src/bot/tools.rs:147` active_tasks：async fn → Vec<Task>（改 → Result<Vec<Task>, String>）
`src-tauri/src/bot/tools.rs:160` tool_list_tasks：pub(crate) async fn → crate::bot::registry::ToolResult（签名不变）
`src-tauri/src/bot/tools.rs:438` tool_complete_task：pub(crate) async fn → ToolResult（签名不变）
`src-tauri/src/bot/tools.rs:472` tool_delete_task：pub(crate) async fn → ToolResult（签名不变）

## 调用点影响清单（5 个以内）

DB-01b（workspace.rs 改 ? 传播）：
1. `src-tauri/src/lib.rs:435` workspace_load 调用方（已 Result 链路）
2. `src-tauri/src/db/workspace.rs:97` upsert_workspace → unchecked（已 Result，validate 链路 OK）

BT-01b（tools.rs active_tasks 改 Result）：
3. `src-tauri/src/bot/registry.rs:252` Box::pin(async move { tool_list_tasks(ctx.app).await }) → dispatch_tool_call 触发（ToolResult 已含 fail 路径）
4. `src-tauri/src/bot/tools.rs:161` tool_list_tasks:3 active_tasks(app).await（接 Result）
5. `src-tauri/src/bot/tools.rs:516/531` tool_complete_task / tool_delete_task 内 active_tasks 调用（同结构改）

总 5 调用点（含 1 dispatch 注册），未超阈值。

## ToolResult 失败处理设计意图（dispatch.rs:402-404 注释原文）

> 失败也走 `ToolResult::ok` 而非 `err`，让 LLM pipeline severity classifier 不把"完成任务失败：DB error"误判为 fatal。

故 BT-01b 失败转换：`active_tasks` Err → `ToolResult::ok(text=format!("查询任务失败：{e}"), refs=vec![], status=ToolStatus::Fail)`。Err 不透传到 MCP 层（设计意图），但函数内部不再 silent-discard。

## 提交信息骨架

```
fix(db,bot): DB-01b-BT-01b — 失败 silent-discard 收口（error-not-propagated family）

【family】error-not-propagated
DB-01b (workspace links 序列化失败 silent-discard 覆写用户数据 → 返 Err)
BT-01b (DB load 失败 silent-discard 返空 Vec 假象 → 返 Err 转 ToolResult::ok fail)
两簇均走 fail-closed，不写"空 Vec"默认值。

【实修 4 处】（2 文件）
- db/workspace.rs:129（DB-01b.1）：unwrap_or_else(|_| "[]".into()) 改 ? 传播
- db/workspace.rs:254（DB-01b.2）：DB-01b.1 同结构改 ? 传播
- bot/tools.rs:147（BT-01b.1）：active_tasks 签名 Vec<Task> → Result<Vec<Task>, String>；body db_load().await.unwrap_or_default() 改 ?
- bot/tools.rs:160（BT-01b.2）：tool_list_tasks 接 active_tasks Err 转 ToolResult::ok(fail)；tool_complete_task:516 / tool_delete_task:531 同结构改

【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1：exit ? / ? comments / ?s（期望 ≤ 6）
【family】error-not-propagated（成员 +5：DB-01b.1/.2 + BT-01b.1/.2a/.2b/.2c）

【拆出】C5-DB-05 (silent mutex poisoning) 不属本批——独立批，涉及 C3-1 约定改不改的产品决策，本批不动
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/DB-01b-BT-01b.spec.md python3 scripts/batch-verify.py docs/batches/DB-01b-BT-01b.spec.md
```