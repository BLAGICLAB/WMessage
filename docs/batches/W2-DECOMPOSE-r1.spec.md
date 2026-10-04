# Batch Spec: W2-DECOMPOSE-r1

## 目的

W2-DECOMPOSE 的 OCR r1 comments 处置批：修复 2 critical + 8 medium + 4 low，
5 条缓期项落 `docs/OCR-FOLLOWUPS-INDEX.md`（W2F-1..W2F-5）。父批 spec：
`docs/batches/W2-DECOMPOSE.spec.md`（首提交 `abe31c5` 已过全套门禁）。

## 修复清单（已修）

- **CRITICAL ×2（同一竞态）**：openWorkflow / createBlank / addNode 未递增
  `decomposeSeqRef`，在途拆解响应会覆盖之后的状态变更——三处补递增
- **medium**：拆解审计失败路径补 `workflow_decompose outcome=failed` 事件；
  指引段服务端长度上限 `MAX_GUIDANCE_CHARS=2000`（invoke 参数不可信任）；
  SettingsPage 提示词保存失败不再显示"已保存"（setDecomposeGuidance 返回 bool）+
  已保存提示 2.5s 自动隐藏 + 卸载兜底持久化 + textarea maxLength；
  重新生成按钮空画布禁用 + regenArmed 随打开/新建清理（3s 定时器泄漏）；
  自动命名改 nameAuto 标志位（字符串比对跨时钟不可靠）+ NAME_AUTO_LEN 具名常量；
  重名后缀改"已占用终名集合"防碰撞（`["审阅","审阅","审阅（2）"]` 不再产出两份"审阅（2）"）；
  武装态按钮补 aria-pressed
- **low**：graph 严格等号相关随文修；契约文本澄清双层上限（20 目标 / 30 硬顶）
- **缓期 5 条** → OCR-FOLLOWUPS-INDEX.md（W2F-1..W2F-5）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W2-DECOMPOSE-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W2-DECOMPOSE-r1.spec.md",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/lib/workflowPrompt.ts",
    "src-tauri/src/workflow_decompose.rs"
  ],
  "max_lines_added": 250,
  "max_lines_removed": 120,
  "findings": [
    {"id": "W2R1-1", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "拆解竞态：三处递增 decomposeSeqRef + regenArmed 清理 + aria-pressed + nameAuto"},
    {"id": "W2R1-2", "file": "src-tauri/src/workflow_decompose.rs", "line": 1, "fix": "失败审计 + 指引段上限 + 重名防碰撞"},
    {"id": "W2R1-3", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1, "fix": "保存反馈/自动隐藏/卸载兜底/maxLength"},
    {"id": "W2R1-4", "file": "src/lib/workflowPrompt.ts", "line": 1, "fix": "setDecomposeGuidance 返回 bool + MAX_GUIDANCE_CHARS"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_decompose.rs": 8
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
