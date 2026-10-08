# Batch Spec: OPEN-UNBIND

## 目的

口径变更（用户拍板）：聊天窗口链接/文件点开不再要求绑定。open_file_path 摘除
绑集/AI_Gen_Files 白名单与 TOCTOU 二次校验；delete_bound_file 仍限定绑集
（破坏性操作另行把关），共用 helper 收窄为 delete 专用（去 gen_dir）。
bot 工具侧授权口径与 capabilities 不动。

## 人类可读摘要

- family: ux-policy-change
- 预估 diff: 2 files modified（files.rs + DEVLOG.md），new 1（本 spec 自身）

## 红线

- delete 侧限制一丝不减；helper 收窄后 delete 行为等价；新增行无批次号 tag

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "OPEN-UNBIND",
  "family": "ux-policy-change",
  "expected_files": [
    "DEVLOG.md",
    "src-tauri/src/bot_skills/files.rs",
    "docs/batches/OPEN-UNBIND.spec.md"
  ],
  "max_lines_added": 220,
  "max_lines_removed": 320,
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
