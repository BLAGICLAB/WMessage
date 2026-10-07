# Batch Spec: B4-CTX-CONSOLIDATE

## 目的

Evolution 批次 B-4（批次 A 文档 §5 唯一适配点）：上下文收拢。
ApplyPolicy 枚举迁入策略层（决策词汇归 strategy）；EvalContext 落地
（applyPolicy/kill_switch/shadow_enabled/now_ms）；apply_from_consolidation
的 kill/notify 分流改读 ctx 谓词——读侧保持 spawn_blocking、频率不变（零行为变更）。

## 人类可读摘要

- family: evolution-batch-b4
- 预估 diff: 4 files / modified +75/-39, new +0

## 红线

- kill 现读现判语义不变（读侧仍 spawn_blocking，每轮一次）
- mod.rs 本批不动（其 auto_allowed 单次读留待 panel 批次）
- 策略层零 IO/时钟/锁/随机（守卫通过）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "B4-CTX-CONSOLIDATE",
  "family": "evolution-batch-b4",
  "expected_files": [
    "docs/batches/B4-CTX-CONSOLIDATE.spec.md",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/policy.rs",
    "src-tauri/src/evolution/strategy.rs"
  ],
  "max_lines_added": 135,
  "max_lines_removed": 99,
  "max_new_files_lines": 200,
  "findings": [
    {
      "id": "B4-applypolicy-move",
      "file": "src-tauri/src/evolution/strategy.rs",
      "line": 16,
      "fix": "ApplyPolicy 枚举迁入策略层（决策词汇归 strategy，policy.rs 转发保路径稳定）"
    },
    {
      "id": "B4-evalctx",
      "file": "src-tauri/src/evolution/strategy.rs",
      "line": 45,
      "fix": "EvalContext 落地（applyPolicy/kill_switch/shadow_enabled/now_ms，纯结构无 IO）"
    },
    {
      "id": "B4-apply-ctx",
      "file": "src-tauri/src/evolution/apply.rs",
      "line": 195,
      "fix": "spawn_blocking 内构造 ctx，kill/notify 分流读 ctx 谓词（读侧仍在 spawn_blocking，频率不变）"
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
