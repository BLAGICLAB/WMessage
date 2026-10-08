# Batch Spec: OCR-TESTS-FIX

## 目的

执行 docs/OCR-TESTS-SCAN-TRIAGE-2026-10-08.md 工单：两个真误绿向量（exec_trace
固定标识/task_chat_exec 固定标题）、四个守卫洞（module_map basename/mod.rs 清单、
layering group use 与泛型 impl）、no_eval 正则收紧、.gitignore PII 漏项、
两处死 sleep、TOCTOU 端口、cleanup 错误浮出。第 16 条按工单跳过。

## 人类可读摘要

- family: ocr-audit-closeout
- 预估 diff: 9 files modified +140/-73, new 1（本 spec 自身）

## 红线

- 测试语义只收紧不放松；selftest 同步钉桩；新增行无批次号 tag

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "OCR-TESTS-FIX",
  "family": "ocr-audit-closeout",
  "expected_files": [
    ".gitignore",
    "DEVLOG.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/tests/bot_test_connection.rs",
    "src-tauri/tests/exec_trace.rs",
    "src-tauri/tests/llm_integration.rs",
    "src-tauri/tests/task_chat_exec.rs",
    "tests-audit/audit_evolution_layering.py",
    "tests-audit/audit_module_map.py",
    "tests-audit/audit_no_eval.py",
    "docs/batches/OCR-TESTS-FIX.spec.md"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 120,
  "max_new_files_lines": 60,
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
