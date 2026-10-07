# Batch Spec: PR3-TYPES-NULL

## 目的

OCR NEEDS-HUMAN 类型修正：F266 filePath/fileIsDir null 语义（跨 invoke 清空显式
传 null）；F275 verdict 字面量 union + 边界归一。

## 人类可读摘要

- family: ocr-needs-types
- 预估 diff: 5 files / +39/-13

## 红线

- 后端零改动（task_patch 的 null=清空/缺键=保留 契约已存在并有测试钉死）
- F275 取值空间经核实不闭合（ReviewReport.verdict String 直传），按 spec 兼容
  分支处理：收紧 + 解析/事件两处边界归一，归一目标与后端降级语义一致

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR3-TYPES-NULL",
  "family": "ocr-needs-types",
  "expected_files": [
    "docs/batches/PR3-TYPES-NULL.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/lib/taskFiles.test.ts",
    "src/lib/taskFiles.ts",
    "src/types.ts"
  ],
  "max_lines_added": 89,
  "max_lines_removed": 63,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F266",
      "file": "src/lib/taskFiles.ts",
      "line": 26,
      "fix": "filesPatch 清空改显式 null（undefined 键被 JSON 丢弃致旧路径复活）；types 放宽 string|null/boolean|null"
    },
    {
      "id": "F275",
      "file": "src/types.ts",
      "line": 174,
      "fix": "verdict 收紧为 ReviewVerdict 字面量 union + 解析/事件两处边界归一（后端 String 直传非闭合）"
    }
  ],
  "assertions_min": {},
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 0
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
