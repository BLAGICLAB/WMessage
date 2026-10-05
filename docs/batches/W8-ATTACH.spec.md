# Batch Spec: W8-ATTACH

## 目的

用户需求：工作流对话框只留「AI 生成」；新增「添加附件」——拆解前 AI 先读附件内容，
把内容拆进各任务卡的备注或子任务。

## 人类可读摘要

- family: workflow-canvas
- 预估 diff: ~8 files / +380/-40 lines
- OCR 计划: r1, timeout 1800s, comments ≤ 5（重点：抽取失败不炸整包、注入长度上限、子任务落库）

## 设计

1. **对话框**（EmptyHero）：goal 输入 + 附件 chips（＋添加附件 → 系统多选文件框，
   chip 可删）+ **只留 AI 生成**（移除「创建空白工作流」按钮——用户拍板；
   createBlank 保留供内部复位路径）
2. **附件持久化**：workflows 表加 `attachments TEXT`（JSON 路径数组，幂等 ALTER），
   Workflow 字段透传；hero 附件列表随打开工作流预填、随保存落库（重新生成可复用）；
   `.wflow.json` 不含附件（机器本地路径跨机无意义）
3. **拆解读附件**：workflow_decompose 新参 attachments；逐个 `bot_py::doc_extract`
   抽文本（用户亲手选 = 明确授权，不走工具授权闸；抽取失败 → 占位说明不炸整包）；
   单文件 12k 字符、总 48k 字符封顶；注入 user 消息【附件 N：文件名】块；
   审计 attachments 数
4. **契约扩展**：节点可选 `"subtasks": ["子任务文本"]`（≤8 条 × ≤60 字）；
   指引段说明"把附件相关内容拆进对应卡的 note 或 subtasks"
5. **落库**：WorkflowNodeDraft.subtasks → **新建卡**构建 Subtask 清单（uuid + 未勾选）；
   保留卡不覆盖子任务（保护执行痕迹，契约不变）；文件格式节点可选 subtasks
   （serde default，老文件零影响）

## 红线

- 附件内容只进 LLM 请求与卡片文本，不落其他存储；抽取失败不阻断拆解
- 保留卡的 subtasks/budget/result 等执行痕迹保护契约不变（指纹不含 subtasks）
- doc_extract 为用户选件直调（不经模型授权闸——授权主体是对话框里的用户本尊）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W8-ATTACH",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W8-ATTACH.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/graph.ts",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/workflow_decompose.rs"
  ],
  "max_lines_added": 480,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W8-1", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "workflows.attachments 列+ALTER+struct/加载/保存；NodeDraft/FileNode subtasks；新卡构建 Subtask"},
    {"id": "W8-2", "file": "src-tauri/src/db/mod.rs", "line": 1, "fix": "workflows.attachments 幂等 ALTER"},
    {"id": "W8-3", "file": "src-tauri/src/workflow_decompose.rs", "line": 1, "fix": "attachments 参数 + doc_extract 抽取封顶注入 + 契约 subtasks"},
    {"id": "W8-4", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "对话框附件 chips + 只留 AI 生成 + 保存链透传"},
    {"id": "W8-5", "file": "src/components/WorkflowCanvas/graph.ts", "line": 1, "fix": "CanvasNode.subtasks + draftFromDecompose 透传"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_decompose.rs": 8
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
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
