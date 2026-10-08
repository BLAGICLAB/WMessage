# Batch Spec: OCR-CLOSEOUT

## 目的

OCR 全量审计收尾批：unlisten 竞态缓解（上游 tauri unlisten_js_script 无洞
守卫的前端侧缓解）+ 3 处同族监听泄漏修复 + observe_run/synthetic 负窗口
fail-fast 补全（回归测试当场抓出既有 checked_sub 修复不完整）+ 文档留档。

## 人类可读摘要

- family: ocr-audit-closeout
- 预估 diff: 12 files modified +159/-45, new +45（本 spec 自身）

## 红线

- 最小外科修复；新注释无批次号 tag；行为变化给可操作中文报错；
  unlistenSafe 仅吞反注册路径失败，不掩盖 listen 阶段真实错误

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "OCR-CLOSEOUT",
  "family": "ocr-audit-closeout",
  "expected_files": [
    "docs/batches/OCR-CLOSEOUT.spec.md",
    "DEVLOG.md",
    "docs/CSP-TIGHTEN-VERIFY-2026-10-07.md",
    "src-tauri/src/bin/observe_run.rs",
    "src-tauri/src/evolution/observe/synthetic.rs",
    "src/App.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/SettingsPage/McpPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/components/WorkspacePage.tsx",
    "src/lib/useTauriListen.ts"
  ],
  "max_lines_added": 220,
  "max_lines_removed": 70,
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
