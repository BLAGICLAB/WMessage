# Batch Spec: B2-APPLY-GATE

## 目的

Evolution 批次 B-2（批次 A 文档迁移顺序第 2 步）：apply.rs 的 gate 与
importance 映射收进策略层 trait；生产调用点（mod.rs gated 过滤）直调 trait。
auto_apply_gate 零逻辑委托 + deprecated（删除期限=批次 C）。

## 人类可读摘要

- family: evolution-batch-b2
- 预估 diff: 4 files / modified +53/-10, new +0
- 测试：importance_maps_high_to_4_others_to_3（新增）；既有 apply/gov 全套绿

## 红线

- 行为零变化（importance High=4/其余=3 原样；gate 白名单语义同 B-1）
- mod.rs 为上下文层：只调用 trait，不 impl（分层守卫）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "B2-APPLY-GATE",
  "family": "evolution-batch-b2",
  "expected_files": [
    "docs/batches/B2-APPLY-GATE.spec.md",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/mod.rs",
    "src-tauri/src/evolution/strategy.rs"
  ],
  "max_lines_added": 113,
  "max_lines_removed": 70,
  "max_new_files_lines": 200,
  "findings": [
    {
      "id": "B2-gate",
      "file": "src-tauri/src/evolution/apply.rs",
      "line": 26,
      "fix": "auto_apply_gate 零逻辑委托 strategy::gate + deprecated"
    },
    {
      "id": "B2-importance",
      "file": "src-tauri/src/evolution/apply.rs",
      "line": 96,
      "fix": "apply_one importance 映射走 trait（High=4/其余=3 原样）"
    },
    {
      "id": "B2-callsite",
      "file": "src-tauri/src/evolution/mod.rs",
      "line": 172,
      "fix": "gated 过滤直调策略 trait（生产调用点迁移）"
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
