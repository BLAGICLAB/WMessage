# Batch Spec: AUDIT-PHASE1A-SKILLDIR-GATE

## 目的

SKILLDIR 批的锁死型测试翻新：capability_tests 旧测试钉「opener scope 严格等于
$APPDATA/**」，scope 移除后 pre-push 拦截。翻新为
`opener_path_scope_must_stay_absent`（allow-open-path 必须不存在 +
opener:default 必须存在），防回退语义保留。

## 人类可读摘要

- family: audit-relay-cleanup
- 预估 diff: 2 files modified（lib.rs + DEVLOG.md），new 1（本 spec 自身）

## 红线

- 只翻测试口径（收紧方向：scope 必须不存在）；新增行无批次号 tag

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "AUDIT-PHASE1A-SKILLDIR-GATE",
  "family": "audit-relay-cleanup",
  "expected_files": [
    "docs/batches/AUDIT-PHASE1A-SKILLDIR-GATE.spec.md",
    "DEVLOG.md",
    "src-tauri/src/lib.rs"
  ],
  "max_lines_added": 120,
  "max_lines_removed": 90,
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
