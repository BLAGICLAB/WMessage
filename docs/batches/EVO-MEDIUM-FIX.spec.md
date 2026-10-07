# Batch Spec: EVO-MEDIUM-FIX

## 目的

同上（含 spec 自身）

## 人类可读摘要

- family: evolution-ocr-medium
- 预估 diff: 4 files / modified +29/-6, new +65

## 红线

- 最小外科修复；新注释无批次号 tag；行为变化给可操作中文报错

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EVO-MEDIUM-FIX",
  "family": "evolution-ocr-medium",
  "expected_files": [
    "docs/batches/EVO-MEDIUM-FIX.spec.md",
    "src-tauri/src/evolution/observe/synthetic.rs",
    "src-tauri/src/evolution/proposal.rs",
    "tests-audit/audit_evolution_layering.py"
  ],
  "max_lines_added": 109,
  "max_lines_removed": 66,
  "max_new_files_lines": 145,
  "findings": [],
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
