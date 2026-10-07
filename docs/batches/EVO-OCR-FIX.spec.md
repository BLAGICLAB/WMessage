# Batch Spec: EVO-OCR-FIX

## 目的

OCR evolution 域聚焦审计（docs/OCR-EVOLUTION-SCAN-2026-10-07.md，134 条）的
critical/high 分诊修复：31 条逐条核查，8 处真实修复 + 8 误报 + 1 已有防线 +
其余 WONTFIX 有据。

## 人类可读摘要

- family: evolution-ocr-fix
- 预估 diff: 7 files / modified +87/-14, new +2512
- 唯一行为变化：E30 mark_human_applied 续走修正（Pending 起点修复 + 中途态
  从下一站续走 + 幂等早退保留）

## 红线

- 最小外科修复；新注释无批次号 tag
- 行为变化仅 E30 一处（可操作中文报错路径保留）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EVO-OCR-FIX",
  "family": "evolution-ocr-fix",
  "expected_files": [
    "docs/OCR-EVOLUTION-SCAN-2026-10-07.md",
    "docs/batches/EVO-OCR-FIX.spec.md",
    "src-tauri/src/evolution/candidate/mapping.rs",
    "src-tauri/src/evolution/change/derive.rs",
    "src-tauri/src/evolution/observe/metrics.rs",
    "src-tauri/src/evolution/panel/commands.rs",
    "tests-audit/audit_evolution_layering.py"
  ],
  "max_lines_added": 147,
  "max_lines_removed": 74,
  "max_new_files_lines": 2600,
  "findings": [
    {
      "id": "E0/E1",
      "file": "tests-audit/audit_evolution_layering.py",
      "line": 147,
      "fix": "分层守卫剥串器生命周期误吞修复 + cfg(test) 单行形态 + selftest 夹具"
    },
    {
      "id": "E30",
      "file": "src-tauri/src/evolution/panel/commands.rs",
      "line": 372,
      "fix": "mark_human_applied 续走起点修正（Pending 全路径/中途态从下一站/幂等早退保留）"
    },
    {
      "id": "E4/E10/E12/E14/E29",
      "file": "src-tauri/src/evolution/candidate/mapping.rs",
      "line": 37,
      "fix": "穷举映射/边界注释统一/debug_assert/分层直连/锁纪律文档等价修正"
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
