# Batch Spec: W10-QA-AUDIT

## 目的

设计 `docs/WORKFLOW-CLARIFY-AUDIT-DESIGN-2026-10-07.md` §4 两块：①**节点级验收 check**——
执行成功且 acceptance 非空的卡，收尾后轻量评审调用对照验收标准核查产出，fail 带证据返工
（独立预算 ≤2），用尽仍 fail 终态改 failed（下游照现语义跳过）；status 与 acceptanceVerdict
分离，下游放行仍只看 status（拍板 4）。②**run 级结构化审计**——`workflow_audit` 表按
`(workflow_id, run_started_at)` 分组（不建 runs 实体表），11 种 kind，尽力而为写入不阻断，
bot.log 保持双写；查询/导出/保留清理 + 设置项。

## 人类可读摘要

- family: workflow-canvas
- 预估 diff: ~14 files / +1300/-40 lines
- OCR 计划: r1, timeout 2400s, comments ≤ 6（重点：验收 fail 不卡死下游语义、返工预算独立、
  审计写失败不阻断、验收豁免口径=用户停止/取消）

## 设计

1. **审计表**（`db/workflow_audit.rs` 新模块）：`workflow_audit(id INTEGER PK AUTOINCREMENT /
   workflow_id TEXT / run_started_at INTEGER / node_task_id TEXT NULL / kind TEXT / level TEXT /
   payload TEXT / created_at INTEGER)`，索引 `(workflow_id, run_started_at)`；
   `wa_insert`（Result 返回，调用方尽力而为）+ `wa_list(workflow_id, limit)` + `wa_prune(keep_runs)`
   （保留最近 N 个 distinct run）+ `wa_clear` + `wa_export`；单测锚点 payload 构造与 prune。
2. **验收 check**（runner）：节点首轮成功（column_done && !fused && result.is_ok）且 acceptance
   非空且开关开且未取消 → 轻量评审单发调用（复用 summarize_messages，全局 active 模型——
   轻量评审模型设置项后置）；`parse_acceptance_verdict` 契约
   `{"verdict":"pass|partial|fail","evidence":"≤100字"}` 剥 fences 解析，坏输出降级 unknown；
   fail → 带 evidence 返工（TaskExecCtx +`rework_evidence`，注入【验收返工】段），attempt 递增，
   **独立预算 ≤2 不与失败重试混用**；用尽仍 fail → result.status 改 failed + ok=false（下游跳过）；
   partial/unknown → status 保持 success。
3. **审计事件**：run_start（trigger=manual|schedule）/ node_start / node_result（status/attempt/ms）/
   acceptance_check / rework / review / rework_round / run_done / stop（RunHandle +run_started_at）/
   question_asked / question_answered（questions 模块双写，payload +runStartedAt）；
   `RunHandle` +`run_started_at`；`workflow_run` +`trigger` 参数（scheduler 传 schedule）。
4. **设置**（`db/workflow_settings.rs` 新模块）：`workflow_settings(key,value)` 表，键
   `node_acceptance`（默认开）/ `audit_retention_runs`（默认 20，钳 5..=100）；
   commands `workflow_settings_get/set`；设置页工作流卡加「验收与审计」段（开关+数字+清空按钮）；
   runner 每次运行读一次开关；保留清理在 run_done 后 + 启动时（lib.rs setup）。
5. **前端**：TaskNode 徽标（result.acceptanceVerdict pass ✓/partial △）；TracePanel 验收行
   （verdict+evidence）+「运行审计」页签（workflow_audit_list 按 run 分组）；画布工具栏
   「审计导出」按钮（save dialog → workflow_audit_export）；types.ts 同步。
6. **非目标**：轻量评审模型设置（v1 跟随 active）、report 逐节点 stats 注入（审计页已覆盖）、
   token 成本汇总。

## 红线

- 下游放行语义不变：`node_is_success`/ok 判定只看 status，验收 verdict 不影响拓扑放行（拍板 4）
- 验收返工预算独立计数（≤2），不占失败重试（C1）额度
- 用户主动停止/取消不做验收（同收尾评审豁免）；验收调用失败降级 unknown 不阻断
- 审计写入尽力而为：单条失败 eprintln 不阻断执行；bot.log 双写不变
- 不建 runs 实体表；`(workflow_id, run_started_at)` 即分组键

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W10-QA-AUDIT",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W10-QA-AUDIT.spec.md",
    "docs/rust-bot-architecture.md",
    "DEVLOG.md",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/workflow_audit.rs",
    "src-tauri/src/db/workflow_settings.rs",
    "src-tauri/src/workflow_runner.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_scheduler.rs",
    "src-tauri/src/workflow_questions.rs",
    "src-tauri/src/lib.rs",
    "src/types.ts",
    "src/lib/workflowAudit.ts",
    "src/components/WorkflowCanvas/TaskNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/TracePanel/TracePanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 1500,
  "max_lines_removed": 60,
  "findings": [
    {"id": "W10-1", "file": "src-tauri/src/db/workflow_audit.rs", "line": 1, "fix": "workflow_audit 单源 DDL + ensure + wa_insert/wa_list/wa_prune/wa_clear + payload 构造单测"},
    {"id": "W10-2", "file": "src-tauri/src/db/workflow_settings.rs", "line": 1, "fix": "workflow_settings 表 + get/set 带默认值 + commands(workflow_settings_get/set, 保留钳 5..=100)"},
    {"id": "W10-3", "file": "src-tauri/src/db/mod.rs", "line": 1, "fix": "两新表 ensure 挂 open_db 迁移链"},
    {"id": "W10-4", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "parse_acceptance_verdict + acceptance_rework_decision 纯函数 + 验收返工环(预算≤2独立) + verdict入result + fail终态ok=false + run/node/acceptance/review/stop 审计事件 + RunHandle.run_started_at + trigger参数 + 保留清理"},
    {"id": "W10-5", "file": "src-tauri/src/bot_chat.rs", "line": 1395, "fix": "TaskExecCtx +rework_evidence + render 注入【验收返工】段"},
    {"id": "W10-6", "file": "src-tauri/src/workflow_questions.rs", "line": 1, "fix": "AskExecContext/Registration +run_started_at; payload +runStartedAt; asked/answered 双写审计表"},
    {"id": "W10-7", "file": "src-tauri/src/lib.rs", "line": 1, "fix": "注册 6 新 command(audit list/clear/clear_all/export + settings get/set) + setup 启动保留清理"},
    {"id": "W10-14", "file": "src-tauri/src/bot_scheduler.rs", "line": 560, "fix": "workflow_run 传 trigger=schedule（spec 对齐：trigger 审计字段需要调度器调用点同步）"},
    {"id": "W10-8", "file": "src/types.ts", "line": 117, "fix": "Task.result +acceptanceVerdict/acceptanceEvidence/attempt/ms; WorkflowAuditEntry/WorkflowSettings 类型"},
    {"id": "W10-9", "file": "src/lib/workflowAudit.ts", "line": 1, "fix": "listAudit/clearAudit/exportAudit/getWorkflowSettings/setWorkflowSettings invoke 封装"},
    {"id": "W10-10", "file": "src/components/WorkflowCanvas/TaskNode.tsx", "line": 1, "fix": "acceptanceVerdict 徽标 pass ✓/partial △（fail 走既有 failed 描边）"},
    {"id": "W10-11", "file": "src/components/TracePanel/TracePanel.tsx", "line": 1, "fix": "验收行(verdict+evidence) + 运行审计页签(workflow_audit_list 按 run 分组)"},
    {"id": "W10-12", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "工具栏「审计导出」按钮(activeId 时可见, save dialog)"},
    {"id": "W10-13", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 3202, "fix": "工作流卡加「验收与审计」段: 节点级验收开关 + 保留次数 + 清空审计"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workflow_audit.rs": 4,
    "src-tauri/src/workflow_runner.rs": 5
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 2400,
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
