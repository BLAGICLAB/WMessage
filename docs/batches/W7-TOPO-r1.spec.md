# Batch Spec: W7-TOPO-r1

## 目的

W7-TOPO 的 OCR r1 处置批：补归一/拓扑重排测试（OCR high：新逻辑零覆盖，成立——
首批提交时测试补丁因锚点失配静默未落）+ value/reason 约定对齐 + Kahn 守卫注释。
父批 spec：`docs/batches/W7-TOPO.spec.md`（首提交 `113cb54`）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W7-TOPO-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W7-TOPO-r1.spec.md",
    "src-tauri/src/workflow_decompose.rs"
  ],
  "max_lines_added": 80,
  "max_lines_removed": 10,
  "findings": [
    {"id": "W7R1-1", "file": "src-tauri/src/workflow_decompose.rs", "line": 1, "fix": "补 2 测试（前向引用重排/自环剥离/真环报名）+ value-reason 约定 + Kahn 守卫注释"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_decompose.rs": 8
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
