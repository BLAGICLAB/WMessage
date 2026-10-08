# Batch Spec: AUDIT-PHASE1A-SKILLDIR

## 目的

权限清单 A3：技能目录「打开目录」在便携模式下被 capability scope（$APPDATA/**）
拒绝——skills_open_dir 改为 Rust 侧 opener 直接打开（与 open_file_path 同方案），
SkillsPanel 删 openPath 依赖，capabilities 删除无消费者的 allow-open-path scope。

## 人类可读摘要

- family: audit-relay-cleanup
- 预估 diff: 5 files modified，new 1（本 spec 自身）

## 红线

- 打开路径由 Rust skills_dir() 决定，非前端传参；capability 只删不加；
  新增行无批次号 tag

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "AUDIT-PHASE1A-SKILLDIR",
  "family": "audit-relay-cleanup",
  "expected_files": [
    "DEVLOG.md",
    "src-tauri/capabilities/default.json",
    "src-tauri/src/bot_skills/manage.rs",
    "src/components/SettingsPage/SkillsPanel.test.tsx",
    "src/components/SettingsPage/SkillsPanel.tsx",
    "docs/batches/AUDIT-PHASE1A-SKILLDIR.spec.md"
  ],
  "max_lines_added": 130,
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
