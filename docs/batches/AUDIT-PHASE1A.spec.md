# Batch Spec: AUDIT-PHASE1A

## 目的

外部整洁度审计（docs/HANDOFF-2026-10-08-zcode.md）阶段 1a：删除 AtomicGuardMiddleware
空骨架（D4d 清空后恒 Allow 的无行为中间件）。连带把「pre_execute 空链 = 漏注册」
的 ERROR 审计断言改为「空链 = 合法态」（否则生产每次工具调用刷 ERROR），
保留空链下原子名单命中的 fail-closed 防线。tool_guard 不动（仍被引用与钉桩）。

## 人类可读摘要

- family: audit-relay-cleanup
- 预估 diff: 2 files modified（middleware.rs + DEVLOG.md），new 1（本 spec 自身）

## 红线

- 行为等价（D4d 后中间件恒 Allow，删除不改变任何工具调用结果）；
  空链防线保留；新增行无批次号 tag（audit-ok 豁免除外）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "AUDIT-PHASE1A",
  "family": "audit-relay-cleanup",
  "expected_files": [
    "DEVLOG.md",
    "src-tauri/src/middleware.rs",
    "docs/batches/AUDIT-PHASE1A.spec.md"
  ],
  "max_lines_added": 160,
  "max_lines_removed": 110,
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
