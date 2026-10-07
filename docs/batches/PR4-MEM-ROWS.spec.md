# Batch Spec: PR4-MEM-ROWS

## 目的

OCR NEEDS-HUMAN F172：update_by_id 返回影响行数，0 行（查改之间被并发删除）
不得假成功。

## 人类可读摘要

- family: ocr-needs-mem-rows
- 预估 diff: 6 files / +52/-7
- 测试：新增 update_by_id_returns_affected_rows（先红后绿）；memory 全套 110 测试绿

## 红线

- store 返回 usize；panel 报「记忆不存在或已被删除」；consolidate 0 行跳过整条
  op（防拿着幻影 target 删 drop_ids 丢真数据）；extract 0 行与存储故障同口径计 failed

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR4-MEM-ROWS",
  "family": "ocr-needs-mem-rows",
  "expected_files": [
    "docs/batches/PR4-MEM-ROWS.spec.md",
    "src-tauri/src/memory/consolidate.rs",
    "src-tauri/src/memory/extract.rs",
    "src-tauri/src/memory/panel.rs",
    "src-tauri/src/memory/store.rs",
    "src-tauri/src/memory/tests.rs"
  ],
  "max_lines_added": 102,
  "max_lines_removed": 57,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F172",
      "file": "src-tauri/src/memory/store.rs",
      "line": 471,
      "fix": "update_by_id 返回影响行数；panel 0 行报不存在；consolidate 0 行跳过 op 防幻影合并；extract 0 行计 failed"
    }
  ],
  "assertions_min": {
    "src-tauri/src/memory/tests.rs": 40
  },
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
