# Batch Spec: C-DEPRECATE-DELETE

## 目的

Evolution 批次 C（破坏性窗口，单独分支 evolution-batch-c）：删除批次 B-1/B-2
遗留的全部 deprecated 委托入口（conflict.rs ×4、change/derive.rs ×1、
apply.rs ×1）。纯删除：不混新逻辑、不混性能优化；原断言改走 trait 路径
保留为回归用例（策略.rs 的 12 例等价穷举已证旧=新）。

## 人类可读摘要

- family: evolution-batch-c
- 预估 diff: 9 files / modified +125/-163, new +0
- shadow.rs 生产调用点（4 filter + 测试）一并直连 trait（总则 6：改调用点
  一次改完并编译验证）

## 红线

- 纯删除；断言实质原样保留；无新逻辑无性能改动
- 删除后全仓 auto_apply_gate/passes_auto_apply_gate/resolve_conflict/
  sort_entries_cross_layer 引用清零（仅存迁移语境注释）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "C-DEPRECATE-DELETE",
  "family": "evolution-batch-c",
  "expected_files": [
    "docs/batches/C-DEPRECATE-DELETE.spec.md",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/candidate/conflict.rs",
    "src-tauri/src/evolution/candidate/mod.rs",
    "src-tauri/src/evolution/change/derive.rs",
    "src-tauri/src/evolution/change/mod.rs",
    "src-tauri/src/evolution/change/record.rs",
    "src-tauri/src/evolution/observe/shadow.rs",
    "src-tauri/src/evolution/strategy.rs"
  ],
  "max_lines_added": 165,
  "max_lines_removed": 203,
  "max_new_files_lines": 150,
  "findings": [
    {
      "id": "C-conflict",
      "file": "src-tauri/src/evolution/candidate/conflict.rs",
      "line": 14,
      "fix": "删 4 个 deprecated 委托（layer_priority/impact_ord/resolve_conflict/sort_entries_cross_layer）；测试改走 trait 断言原样保留"
    },
    {
      "id": "C-changederive",
      "file": "src-tauri/src/evolution/change/derive.rs",
      "line": 36,
      "fix": "删 passes_auto_apply_gate；from_proposal 直连 trait；测试改走 trait"
    },
    {
      "id": "C-apply-shadow",
      "file": "src-tauri/src/evolution/apply.rs",
      "line": 26,
      "fix": "删 auto_apply_gate；测试改走 trait；shadow.rs 5 处调用点直连 trait（gate_approved 本地谓词）"
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
