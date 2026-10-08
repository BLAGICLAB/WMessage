# Batch Spec: W1-CSP-REPORT

## 目的

同上（含 spec 与全部工作流文件）

## 人类可读摘要

- family: csp-report
- 预估 diff: 7 files / modified +72/-1, new +259

## 红线

- 最小外科修复；新注释无批次号 tag；行为变化给可操作中文报错

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W1-CSP-REPORT",
  "family": "csp-report",
  "expected_files": [
    "docs/batches/W1-CSP-REPORT.spec.md",
    "scripts/test-all.sh",
    "src-tauri/src/audit.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/tauri.conf.json",
    "src/main.tsx",
    "tests-audit/audit_no_eval.py"
  ],
  "max_lines_added": 152,
  "max_lines_removed": 61,
  "max_new_files_lines": 339,
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
