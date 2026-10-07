# Batch Spec: PR12-CSP-VERIFY-PREP

## 目的

F10 CSP 收紧验证的**准备批**：六类间接 eval 补扫报告 + 冒烟运行手册 + 临时构建
覆盖配置（仅 csp 去 unsafe-eval）+ 两个 debug 构建产物（gitignored，不入库）。
**主配置 tauri.conf.json 本批零改动**；冒烟由人工按手册执行，checklist 全勾并
复核后才另开 commit 改主配置。

## 人类可读摘要

- family: csp-tighten-verify
- 预估 diff: 3 files（1 文档 + 1 临时配置 + 1 spec）
- 构建产物验证：baseline 二进制嵌含 unsafe-eval 的 csp；tightened 二进制 csp 已收紧、
  devCsp 原样（strings 提取核对）

## 红线

- 主配置零改动（本批 git diff 主配置必须为空）
- devCsp 不动不测（tauri-2.11.5 源码实证：构建产物只用 csp）
- 结论表述按任务书弱化：静态无法排除极端别名混淆，运行时采集兜底

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR12-CSP-VERIFY-PREP",
  "family": "csp-tighten-verify",
  "expected_files": [
    "docs/CSP-TIGHTEN-VERIFY-2026-10-07.md",
    "docs/batches/PR12-CSP-VERIFY-PREP.spec.md",
    "src-tauri/tauri.csp-verify.json"
  ],
  "max_lines_added": 320,
  "max_lines_removed": 5,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F10",
      "file": "src-tauri/tauri.csp-verify.json",
      "line": 3,
      "fix": "CSP 收紧验证的临时构建覆盖（仅 csp 去 unsafe-eval）；主配置未动，冒烟由人工执行后复核"
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
