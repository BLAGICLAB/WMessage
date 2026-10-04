# Batch Spec: W3-RUNNER-r1

## 目的

W3-RUNNER 的 OCR r1 comments 处置批：11 条（2H+5M+4L）全部随批修复，无缓期项。
父批 spec：`docs/batches/W3-RUNNER.spec.md`（首提交 `f7febcf` 已过全套门禁）。

## 修复清单（已修）

- **HIGH**：开始执行按钮 title 嵌套三元 → `runButtonTitle()` 具名 helper
- **HIGH**：openWorkflow 的 is_running 响应无竞态守卫 → openSeqRef 序号比对
- **medium**：GoalNode 包装 div 恒渲染（空 margin 回归）→ 条件渲染
- **medium**：should_emit 的 Workflow 臂无测试 → should_emit_workflow_same_as_scheduled；
  文档注释补 Workflow 条目
- **medium**：stopRun 无防重入且不复位 → stopBusyRef + 300ms 后立即回查复位
- **medium**：切换工作流 running 状态错乱 → openWorkflow 查询带守卫（取真）+
  createBlank 归零（W1 已有，复核确认）
- **medium**：setRunning(true) 早于后端登记 → 成功后才置真 + startBusyRef 防连点
- **low**：轮询吞错无限轮 → 连续 3 次失败自愈复位按钮
- **low**：`workflow_is_running_cmd` 命名不合惯例 → 命令改名 `workflow_is_running`，
  内部函数改 `runner_is_running`（lib.rs 注册/前端 invoke 同步）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W3-RUNNER-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W3-RUNNER-r1.spec.md",
    "src/components/WorkflowCanvas/GoalNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/bot_artifacts.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/workflow_runner.rs"
  ],
  "max_lines_added": 160,
  "max_lines_removed": 60,
  "findings": [
    {"id": "W3R1-1", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "title helper + is_running 竞态守卫 + start/stop 收口 + 轮询自愈"},
    {"id": "W3R1-2", "file": "src/components/WorkflowCanvas/GoalNode.tsx", "line": 1, "fix": "条件渲染空 margin"},
    {"id": "W3R1-3", "file": "src-tauri/src/bot_artifacts.rs", "line": 1, "fix": "Workflow 臂测试 + 文档"},
    {"id": "W3R1-4", "file": "src-tauri/src/lib.rs", "line": 1, "fix": "命令改名注册"},
    {"id": "W3R1-5", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "命令/内部函数改名"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_runner.rs": 8
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```
