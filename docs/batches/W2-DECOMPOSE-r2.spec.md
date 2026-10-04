# Batch Spec: W2-DECOMPOSE-r2

## 目的

W2-DECOMPOSE 的 OCR r2（对 r1 修复增量 `abe31c5..04b2072` 复验）comments 处置：
6 修复 + 1 误报定性。r2 报告（本地）：`docs/OCR-CODE-REVIEW-2026-10-04-w2-r2.json`
（7 条：1H + 4M + 2L；终态 0 未处置 H/C）。

## 处置清单

- **HIGH（误报定性）**：重名去重"三次同名产出 审阅（2）（2）"——逐行追踪不成立：
  `base` 取自 `st.title` 的**当前输入**（未被先前迭代改写；后缀写在迭代末尾），
  现有测试 `validate_suffixes_duplicate_titles` 断言的正是该 high 声称会错的
  `["审阅","审阅（2）","审阅（3）"]` 且通过。为锁死行为另补
  `dedupe_collision_safe_with_presuffixed_input`（输入自带"（2）"时终名仍两两不同）
- **medium**：LLM 调用本身的失败经 `?` 直抛绕过审计 → 统一收口 `fail!` 宏，
  成功/校验失败/调用失败三路都有 `workflow_decompose` 审计事件，错误值走
  `escape_for_log` 管道；`MAX_GUIDANCE_CHARS` 边界测试锁定（上限拒/上限-1 过/空白 trim，
  顺带抽 `validate_guidance` 纯函数）；openWorkflow 补 `setNameAuto(false)`
  （已保存工作流的名称是作者起的，重新生成不得覆盖）；addNode 补 `setRegenArmed(false)`

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W2-DECOMPOSE-r2",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W2-DECOMPOSE-r2.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/workflow_decompose.rs"
  ],
  "max_lines_added": 120,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W2R2-1", "file": "src-tauri/src/workflow_decompose.rs", "line": 1, "fix": "失败审计三路收口 + escape_for_log + guidance 边界测试 + 去重边界测试"},
    {"id": "W2R2-2", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "openWorkflow setNameAuto(false) + addNode 清 regenArmed"}
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
