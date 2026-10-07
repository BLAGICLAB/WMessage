# Batch Spec: PR8-LESSON-TUNING

## 目的

OCR NEEDS-HUMAN F179：record_lesson_core 原走 insert_item 默认参数，用户设置的
memoryTuning（容量/去重阈值）对教训写入不生效。拍板「必填改签名」：加必填
sp: &StoreParams，一次改完全部调用点。

## 人类可读摘要

- family: ocr-needs-lesson-tuning
- 预估 diff: 4 files / +91/-13
- 测试：record_lesson_core_respects_tuning_capacity（capacity=1 第二条挤掉
  第一条）；9 处既有测试调用补 StoreParams::default()

## 红线

- 生产调用（bot 工具 / 任务失败教训 / skill 失败教训 ×3）统一读
  read_memory_tuning → StoreParams::of，与 remember 路径同口径
- 无适配器旧函数残留（必填口径一次到位，拍板 #2）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR8-LESSON-TUNING",
  "family": "ocr-needs-lesson-tuning",
  "expected_files": [
    "docs/batches/PR8-LESSON-TUNING.spec.md",
    "src-tauri/src/bot_skills/runtime.rs",
    "src-tauri/src/memory/mod.rs",
    "src-tauri/src/memory/tests.rs"
  ],
  "max_lines_added": 141,
  "max_lines_removed": 63,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F179",
      "file": "src-tauri/src/memory/mod.rs",
      "line": 654,
      "fix": "record_lesson_core 加必填 tuning 参数走 insert_item_with（拍板=必填改签名）；生产调用读 memoryTuning，9 处测试传 default"
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
