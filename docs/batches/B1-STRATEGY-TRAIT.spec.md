# Batch Spec: B1-STRATEGY-TRAIT

## 目的

Evolution 批次 B-1（批次 A 文档 §5/§11.3，已获批）：策略 trait 首落地。
strategy.rs（trait + 默认实现）+ conflict.rs / change/derive.rs 零逻辑委托
（deprecated 标删除期限=批次 C）+ 分层依赖方向 CI 守卫（selftest+误报处理）。
测试：12 例等价穷举 + 快照对照 + 理由断言 + 稳定排序。

## 人类可读摘要

- family: evolution-batch-b1
- 预估 diff: 3 files / modified +15/-25, new +0
- 事实：被迁函数现无生产调用方（候选池消解实验态待接线），迁移零回归面

## 红线

- 旧函数体只允许一行委托（不变式 10）；不删旧符号不改旧签名（纯增量可回滚）
- strategy.rs 纯度：无 IO/时钟/锁/随机源/tauri（分层守卫）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "B1-STRATEGY-TRAIT",
  "family": "evolution-batch-b1",
  "expected_files": [
    "docs/batches/B1-STRATEGY-TRAIT.spec.md",
    "docs/batches/B1-STRATEGY-TRAIT.spec.md",
    "src-tauri/src/evolution/strategy.rs"
  ],
  "max_lines_added": 95,
  "max_lines_removed": 85,
  "max_new_files_lines": 60,
  "findings": [
    {
      "id": "B1-trait",
      "file": "src-tauri/src/evolution/strategy.rs",
      "line": 1,
      "fix": "EvolutionPolicy trait + DefaultEvolutionPolicy（gate/resolve/order_entries，纯策略）"
    },
    {
      "id": "B1-delegate-conflict",
      "file": "src-tauri/src/evolution/candidate/conflict.rs",
      "line": 16,
      "fix": "四函数零逻辑委托 + deprecated（删除期限=批次 C）"
    },
    {
      "id": "B1-delegate-changederive",
      "file": "src-tauri/src/evolution/change/derive.rs",
      "line": 36,
      "fix": "passes_auto_apply_gate 零逻辑委托 gate（白名单形式，等价 12 例钉死）"
    },
    {
      "id": "B1-ci",
      "file": "tests-audit/audit_evolution_layering.py",
      "line": 1,
      "fix": "分层依赖方向守卫（selftest+误报处理），注册 test-all"
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
