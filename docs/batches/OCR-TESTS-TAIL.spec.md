# Batch Spec: OCR-TESTS-TAIL

## 目的

OCR-TESTS-FIX 尾巴：工单第 16 条 skill_e2e DDL 单源化（SKILL_OUTCOMES_DDL
单源常量）+ mock_llm 9 处死 sleep 删除（#46 同模式收尾）+ memory_eval
recall@5/top_n 耦合锁死（#84）。exec_trace 两处 RAII guard 未用警告顺手消。

## 人类可读摘要

- family: ocr-audit-closeout
- 预估 diff: 7 files modified，+60/-40 左右，new 1（本 spec 自身）

## 红线

- 测试语义只收紧不放松；DDL 单源后生产/测试建表必须同一常量；
  新增行无批次号 tag

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "OCR-TESTS-TAIL",
  "family": "ocr-audit-closeout",
  "expected_files": [
    "DEVLOG.md",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/skill_out.rs",
    "src-tauri/tests/exec_trace.rs",
    "src-tauri/tests/memory_eval.rs",
    "src-tauri/tests/mock_llm.rs",
    "src-tauri/tests/skill_e2e.rs",
    "docs/batches/OCR-TESTS-TAIL.spec.md"
  ],
  "max_lines_added": 150,
  "max_lines_removed": 100,
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
