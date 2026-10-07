# Batch Spec: PR12-CSP-FLIP

## 目的

F10 CSP 收紧落地：主配置 `csp` 仅移除 script-src 的 `'unsafe-eval'`（`devCsp`
与其余指令一字不动）。前置证据：docs/CSP-TIGHTEN-VERIFY-2026-10-07.md
（静态六类补扫 + 浏览器层零违规 + WebView 冒烟 16 项 checklist 全勾 + 人工确认）。

## 人类可读摘要

- family: csp-tighten-verify
- 预估 diff: 2 files / 1 行配置改动
- 回滚：恢复 script-src 里 `'unsafe-eval'` 一词（单行 revert）

## 红线

- 本 commit 即任务书 §七 的配置改动步；`devCsp` 未覆盖未修改

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR12-CSP-FLIP",
  "family": "csp-tighten-verify",
  "expected_files": [
    "docs/batches/PR12-CSP-FLIP.spec.md",
    "src-tauri/tauri.conf.json"
  ],
  "max_lines_added": 30,
  "max_lines_removed": 5,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F10",
      "file": "src-tauri/tauri.conf.json",
      "line": 21,
      "fix": "csp 仅移除 script-src 的 'unsafe-eval'（冒烟通过+人工确认后的落地 commit）"
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
