# Batch Spec: W8-ATTACH-r1

## 目的

W8-ATTACH 的 OCR r1 comments 处置批：2C + 1H + 7M + 2L 随批修复，5 条缓期落账
（W8F-1..5）。父批 spec：`docs/batches/W8-ATTACH.spec.md`（首提交 `7489701`）。

## 修复清单（已修）

- **CRITICAL**：新卡子任务被静默丢弃——workflow_save_locked 的 Task 字面量仍
  `subtasks: None`（首轮构建补丁落错位置）→ 在真实字面量处从草稿构建 Subtask
  （uuid + 未勾选），并删除误落的旧块
- **CRITICAL**：dirty 快照不含 attachPaths——附件增删永远不亮保存键 →
  snapshot/save/openWorkflow 三处快照全部带上 attachPaths
- **HIGH**：openWorkflow 在 seq 检查前 setAttachPaths（快速切换泄漏）→ 移到检查后
- **HIGH**：附件数无上限（每个触发 doc_extract 子进程）→ 命令入口拒 >10
- **medium**：附件序列化 unwrap_or_default 吞错 → map_err 传播；
  ensure_workflows_attachments 的 unwrap_or(false) → 传播错误（fail-closed）；
  total_used 截断后未更新（计数修正）+ 空文本不计成功；
  basename 跨平台（'/' 与 '\\' 双分隔）；
  validate_nodes 补子任务校验（与拆解侧同规则 ≤8×60）；
  EmptyHero 文案去掉已移除的空白画布指引
- **low**：847 注释修正；>8 条错误 value 规范、子任务错误带条目序号
- **缓期 5 条** → W8F-1..5

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W8-ATTACH-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W8-ATTACH-r1.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/workflow_decompose.rs"
  ],
  "max_lines_added": 220,
  "max_lines_removed": 60,
  "findings": [
    {"id": "W8R1-1", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "新卡 subtasks 真实字面量构建 + serialize/ALTER 错误传播 + validate_nodes 子任务校验 + 注释"},
    {"id": "W8R1-2", "file": "src-tauri/src/workflow_decompose.rs", "line": 1, "fix": "附件数上限 10 + basename 跨平台 + 计数修正 + 子任务错误序号"},
    {"id": "W8R1-3", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "dirty 含 attachPaths（三处快照）+ seq 后置附件 + 文案"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workflow.rs": 26
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
