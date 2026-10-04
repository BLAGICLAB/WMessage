# Batch Spec: W3-RUNNER-r2

## 目的

W3-RUNNER 的 OCR r2（对 r1 修复增量 `f7febcf..f97aaab` 复验）comments 处置：
2 medium 同指 stopRun 回查窗口，一并修复。r2 报告（本地）：
`docs/OCR-CODE-REVIEW-2026-10-04-w3-r2.json`（2 条，0H/0C 终态）。

## 修复清单

- **medium**：stopBusyRef 在 finally 立即解除（早于 300ms 回查）→ 推迟到 700ms 定时解除，
  回查窗口内不再可能重复点击
- **medium**：300ms 回查闭包捕获的 activeId 会跨工作流切换漂移 → 绑定被停的 `wf`
  局部变量 + openSeqRef 快照双重守卫（已切换时本回查不应用，切换路径各有自己的
  running 取真/归零）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W3-RUNNER-r2",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W3-RUNNER-r2.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 20,
  "findings": [
    {"id": "W3R2-1", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "stopRun 回查窗口：busy 推迟解除 + wf 绑定 + seq 守卫"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_runner.rs": 8
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
