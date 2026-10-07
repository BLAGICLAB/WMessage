# Batch Spec: B1-STRATEGY-TRAIT

## 目的

Evolution 批次 B-1（批次 A 文档 §5/§11.3）：策略 trait 首落地。
strategy.rs（trait + 默认实现）+ conflict.rs / change/derive.rs 零逻辑委托
（#[deprecated] 标删除期限=批次 C）+ 分层依赖方向 CI 守卫。
测试：12 例等价穷举 + 快照对照 + 理由断言 + 稳定排序，先红后绿。

## 人类可读摘要

- family: evolution-batch-b1
- 预估 diff: 8 files / modified +51/-44, new +604
- 事实：被迁六函数现无生产调用方（候选池消解待接线，conflict.rs 模块头
  实验态登记），迁移零回归面；worktree 隔离验证全量 1609 绿

## 红线

- 旧函数体只允许一行委托（不变式 10）；不删旧符号不改旧签名（批次 B 纯增量可回滚）
- strategy.rs 纯度：无 IO/时钟/锁/随机源/tauri（audit_evolution_layering 守卫）
- 委托壳行级豁免仅限受认可调用形式；本批含并行会话 W11-OCR 对 mod.rs 的
  恢复注释（其 staged mod.rs 含我的模块注册行，防 reset 事故再丢，一并入库）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "B1-STRATEGY-TRAIT",
  "family": "evolution-batch-b1",
  "expected_files": [
    "docs/batches/B1-STRATEGY-TRAIT.spec.md",
    "docs/rust-bot-architecture.md",
    "scripts/test-all.sh",
    "src-tauri/src/evolution/candidate/conflict.rs",
    "src-tauri/src/evolution/change/derive.rs",
    "src-tauri/src/evolution/mod.rs",
    "src-tauri/src/evolution/strategy.rs",
    "tests-audit/audit_evolution_layering.py"
  ],
  "max_lines_added": 111,
  "max_lines_removed": 104,
  "max_new_files_lines": 800,
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
      "fix": "四函数零逻辑委托 + #[deprecated]（删除期限=批次 C）"
    },
    {
      "id": "B1-delegate-changederive",
      "file": "src-tauri/src/evolution/change/derive.rs",
      "line": 36,
      "fix": "passes_auto_apply_gate 零逻辑委托 gate（白名单形式收敛，等价 12 例钉死）"
    },
    {
      "id": "B1-ci",
      "file": "tests-audit/audit_evolution_layering.py",
      "line": 1,
      "fix": "分层依赖方向守卫（策略纯度/上下文边界/数据层方向/禁随机源，selftest+误报处理）"
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
