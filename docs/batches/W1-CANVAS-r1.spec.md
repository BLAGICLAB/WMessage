# Batch Spec: W1-CANVAS-r1

## 目的

W1-CANVAS 的 OCR r1 comments 处置批：修复 3 critical + 2 high + 8 处低成本 medium/low，
其余 11 条缓期项落 `docs/OCR-FOLLOWUPS-INDEX.md`（W1F-1..W1F-11）。父批 spec：
`docs/batches/W1-CANVAS.spec.md`（首提交 `1259af1` 已过全套门禁）。

## 人类可读摘要

- family: workflow-canvas（r1 处置）
- 覆盖 findings: OCR r1 报告 `docs/OCR-CODE-REVIEW-2026-10-04-w1.json`（41 条，19 文件）
- 预估 diff: 12 files / +228/-155
- OCR 计划: r2（对 r1 修复增量复验），timeout 1800s，期望 comments ≤ 3

## 修复清单（已修）

- **CRITICAL ×3**：可见性事件系统错配——dispatch 改 Tauri `emit()`（跨 webview），
  订阅端既有 Tauri `listen` 保持；localStorage 写失败不再派发误导事件
- **HIGH**：workflow_save 广播 TOCTOU——`workflow_save_locked` 改返回
  `WorkflowSaveOutcome { result, upserts, deleted_ids }`，广播载荷 = 锁内写定行，
  删除锁外二次 open_db 重读；顺带修 load_workflow 双查
- **HIGH**：rfNodes 闭包过期——commitTitle/toggleDone/toggleSubtask/deleteNode
  useCallback + tasksRef/propsRef（useEffect 同步），memo deps 补全回调
- **medium**：selectedIds 随节点删除裁剪；保存后 name/goal 与后端 trim 对齐（dirty 恒真）；
  deleteArmed 定时器 ref 清理；openWorkflow/openSeqRef 竞态守卫；
  W1_TASK_COLUMNS 单源化（open_db 迁移与 legacy fixture 共用）；
  WidgetApp 过滤改用 isWorkflowTask；TaskNode 嵌套三元拆函数 + `!==` + 去空操作回调；
  graph.ts 死代码 adj 移除 + Set 查询；设置开关 role=switch/aria-checked/aria-label；
  reloadTasks 失败不再 silent
- **缓期 11 条** → OCR-FOLLOWUPS-INDEX.md（W1F-1..W1F-11，逐条含处置理由）

## 红线

- family 一致：只处置 W1-CANVAS r1 comments，不混入新功能
- 行为零变化目标：所有修复不改变已验证的保存语义（指纹 diff）与过滤语义

## 自主执行规则

spec 起草即视为 reviewer 批准（自主模式），agent 全权执行至 commit。
完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + follow-up。

## Stop 条件（触发即停，报 reviewer）

- compile_failure / architecture_blocker / family_heterogeneity /
  new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W1-CANVAS-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W1-CANVAS.spec.md",
    "docs/batches/W1-CANVAS-r1.spec.md",
    "src/App.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/components/WorkflowCanvas/TaskNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/graph.ts",
    "src/lib/workflowVisibility.ts",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow.rs"
  ],
  "max_lines_added": 300,
  "max_lines_removed": 200,
  "findings": [
    {"id": "W1R1-1", "file": "src/lib/workflowVisibility.ts", "line": 1, "fix": "事件错配改 Tauri emit"},
    {"id": "W1R1-2", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "广播载荷锁内写定，去锁外重读"},
    {"id": "W1R1-3", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "回调 useCallback+ref 防过期闭包"},
    {"id": "W1R1-4", "file": "src/components/WidgetApp/WidgetApp.tsx", "line": 1, "fix": "过滤复用 isWorkflowTask"},
    {"id": "W1R1-5", "file": "src/components/WorkflowCanvas/TaskNode.tsx", "line": 1, "fix": "三元拆函数/严格等号/去空回调"},
    {"id": "W1R1-6", "file": "src/components/WorkflowCanvas/graph.ts", "line": 1, "fix": "死代码移除/Set 查询"},
    {"id": "W1R1-7", "file": "src/App.tsx", "line": 1, "fix": "reloadTasks 失败可见"},
    {"id": "W1R1-8", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1, "fix": "开关 ARIA"},
    {"id": "W1R1-9", "file": "src-tauri/src/db/mod.rs", "line": 1, "fix": "W1 列清单单源"},
    {"id": "W1R1-10", "file": "src-tauri/src/db/tasks.rs", "line": 1, "fix": "W1_TASK_COLUMNS 常量"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workflow.rs": 8
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
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
