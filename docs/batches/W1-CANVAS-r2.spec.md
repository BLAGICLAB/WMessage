# Batch Spec: W1-CANVAS-r2

## 目的

W1-CANVAS 的 OCR r2（对 r1 修复增量 `1259af1..62ff334` 复验）comments 处置：
1 medium + 7 low 全量随批修/落账。r2 报告：`docs/OCR-CODE-REVIEW-2026-10-04-w1-r2.json`
（8 条，0 critical/high——W4 收口红线「无未处置 high/critical」已满足）。

## 修复清单

- **medium**：nodeBorder 行为回归——`column=done` 无 result（用户手动完成）必须给绿环；
  仅 `result.status` 明确非 success（超时/异常）才红
- **low**：TaskNode/graph.ts 严格等号（`!==`/`=== undefined`）；`W1_TASK_COLUMNS`
  收紧为 `pub(crate)`；workflow.rs 两个 Vec 加 `with_capacity`；测试错误路径抽 `r()` helper

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W1-CANVAS-r2",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W1-CANVAS-r2.spec.md",
    "src/components/WorkflowCanvas/TaskNode.tsx",
    "src/components/WorkflowCanvas/graph.ts",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow.rs"
  ],
  "max_lines_added": 80,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W1R2-1", "file": "src/components/WorkflowCanvas/TaskNode.tsx", "line": 30, "fix": "done 无 result 回归绿环"},
    {"id": "W1R2-2", "file": "src/components/WorkflowCanvas/graph.ts", "line": 1, "fix": "严格等号"},
    {"id": "W1R2-3", "file": "src-tauri/src/db/tasks.rs", "line": 1, "fix": "pub(crate) 收紧"},
    {"id": "W1R2-4", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "with_capacity + 测试 helper"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workflow.rs": 8
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 1800,
    "expected_max_comments": 0
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```
