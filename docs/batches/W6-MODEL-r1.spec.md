# Batch Spec: W6-MODEL-r1

## 目的

W6-MODEL 的 OCR r1 comments 处置批：1C + 2H + 5M + 2L 随批修复，3 条缓期落账
（W6F-1..3）。父批 spec：`docs/batches/W6-MODEL.spec.md`（首提交 `bc72074`）。

## 修复清单（已修）

- **CRITICAL + HIGH（同根：保存/草稿链丢 model）**：CanvasNode 增加 model 字段
  （draftFromTasks 透传）→ 保存 payload 携带 model → kept 卡服务端同步 model
  （草稿为准，同 canvasPos 语义）→ onModelChange 改走 useCallback（setNodes +
  onUpdate 双写，原只 patch 任务会在保存时被草稿回退）
- **HIGH**：推理参数映射错位——覆盖命中时 provider/model_for_reasoning/推理整组
  （temperature/top_p/system_prompt/max_tokens 钳制）走覆盖条目（schema 新增
  `inference_for_entry` 条目级推理，effective_inference 重构复用），防跨协议钳制错位
- **medium**：parse 空 model 归 None（与 patch 拒空串契约对齐）｜模型下拉过滤停用条目
  （与聊天面板同规则）｜模型库加载失败走 silent 弹窗模式｜DDL 注释 27→28 列
- **low**：已设模型但条目被删时下拉仍显示（当前 id 补为「条目已删除」选项）；
  graph 测试透传随类型自然覆盖
- **缓期 3 条** → W6F-1..3（钥匙串分支不测 / hygiene 顺手修 / 非对话模型条目属产品文档）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W6-MODEL-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W6-MODEL-r1.spec.md",
    "src/components/WorkflowCanvas/TaskNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/graph.ts",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/schema.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow.rs"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 60,
  "findings": [
    {"id": "W6R1-1", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "保存链携带 model + changeModel 双写 + 过滤停用 + catch 模式"},
    {"id": "W6R1-2", "file": "src-tauri/src/bot/config/types.rs", "line": 1, "fix": "ResolvedModel 推理字段 + 三参签名 + 测试"},
    {"id": "W6R1-3", "file": "src-tauri/src/bot_model_loop.rs", "line": 1, "fix": "覆盖命中时推理整组走覆盖条目"},
    {"id": "W6R1-4", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "kept 卡同步 model + parse 空 model 归 None"},
    {"id": "W6R1-5", "file": "src-tauri/src/bot/config/schema.rs", "line": 1, "fix": "inference_for_entry 条目级推理"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/config/types.rs": 4
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
