# Batch Spec: G7-SIZEMODE 节点大小双模式（连接度/耗时）+ created_at 字段全链路

```json
{
  "batch_id": "G7-SIZEMODE",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G7-SIZEMODE.spec.md",
    "DEVLOG.md",
    "gui-test-screenshots/graph-demo-entry.tsx",
    "src/types.ts",
    "src/storage.ts",
    "src/storage.test.ts",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/GraphPage/graph-adapter.ts",
    "src/components/GraphPage/graph-adapter.test.ts",
    "src/components/GraphPage/graph-build.ts",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/task_out.rs",
    "src-tauri/src/workflow_runner.rs"
  ],
  "max_lines_added": 430,
  "max_lines_removed": 70,
  "findings": [
    {"id": "G7-1", "file": "src-tauri/src/db/tasks.rs", "line": 29, "fix": "tasks 表加 created_at 列（幂等 ALTER + DDL + SELECT/INSERT/row 映射）；UPDATE SET 刻意不含（创建时间只插入不更新，照 workflows 先例）；apply_task_patch 受保护字段加 createdAt"},
    {"id": "G7-2", "file": "src-tauri/src/db/mod.rs", "line": 215, "fix": "open_db 幂等 ALTER 循环 + legacy fixture 第三循环 + 回归锁 upsert_insert_stamps_created_at_update_never_overwrites"},
    {"id": "G7-3", "file": "src/components/GraphPage/graph-adapter.ts", "line": 62, "fix": "GraphSizeMode 双模式：degree=√连接度（现状默认）；duration=耗时 3+1.5·√天数 15 封顶（done=完成−创建，doing=现在−创建，todo/缺创建时间=最小 3）"},
    {"id": "G7-4", "file": "src/storage.ts", "line": 100, "fix": "diffTaskRows 新行兜底打 createdAt=now（防未来新建入口忘打戳；存量行变更不补）"},
    {"id": "G7-5", "file": "src/components/GraphPage/GraphPage.tsx", "line": 53, "fix": "图例区「大小：连接度/耗时」切换 + localStorage wm.graph.sizeMode 持久化（设置页批次落地后读同一键）"}
  ],
  "assertions_min": {
    "src-tauri/src/db/mod.rs": 1
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 1
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

老板拍板 C 方案：图谱节点大小可切换「连接度 / 耗时」（耗时 = 完成时间−创建时间
天数；doing 用已进行天数——钉子户可视化）。前提：tasks 表此前无创建时间字段，
本批补 created_at 全链路（建库/迁移/结构体/upsert/load/四个新建入口打戳/
导出导入 serde 透传/前端 diffTaskRows 兜底），老数据 NULL=未知按最小尺寸。
