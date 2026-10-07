# Batch Spec: PR1-TEST-HYGIENE

## 目的

OCR NEEDS-HUMAN 三条测试卫生：集成测试清场改 Drop guard，失败路径清得到、panic 不残留。

## 人类可读摘要

- family: ocr-needs-test-hygiene
- 预估 diff: 3 files / +0/-0

## 红线

- 最小修复；先补测试再改实现；禁止顺手重构

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR1-TEST-HYGIENE",
  "family": "ocr-needs-test-hygiene",
  "expected_files": [
    "docs/batches/PR1-TEST-HYGIENE.spec.md",
    "src-tauri/tests/evolution_gov.rs",
    "src-tauri/tests/exec_trace.rs",
    "src-tauri/tests/skill_e2e.rs"
  ],
  "max_lines_added": 213,
  "max_lines_removed": 56,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F213",
      "file": "src-tauri/tests/evolution_gov.rs",
      "line": 59,
      "fix": "cleanup+restore_cfg 包 Drop guard，panic 展开也清场"
    },
    {
      "id": "F225",
      "file": "src-tauri/tests/exec_trace.rs",
      "line": 65,
      "fix": "trace 家族补按 task_id 删除（失败路径 session 对不上）+ Drop guard"
    },
    {
      "id": "F234",
      "file": "src-tauri/tests/skill_e2e.rs",
      "line": 61,
      "fix": "rebuild_routes 还原改 Drop guard（断言 panic 也还原）"
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
