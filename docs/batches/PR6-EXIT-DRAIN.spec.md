# Batch Spec: PR6-EXIT-DRAIN

## 目的

OCR NEEDS-HUMAN F163：api_stop_for_exit 不等在途 handler，退出可能截断请求。
按拍板实现退出排空：拒新 → 等在飞归零（30s 默认，WM_API_EXIT_DRAIN_SECS 覆盖）
→ 超时 WARN 强退。in_flight 计数复用 A6 并发上限的 active 计数器（非新造）。

## 人类可读摘要

- family: ocr-needs-exit-drain
- 预估 diff: 4 files / +133/-8
- 测试：exit_drains_in_flight_handler_then_refuses_new（先红后绿：store 自限时
  300ms，stop 返回前 released 必置位 + 耗时 ≥250ms + 退出后连接被拒）

## 红线

- api_stop（用户关）/ api_rotate_token 行为零变化（不等待，只透传计数器）
- 超时字段：in_flight 数 + timeout_secs 进 WARN 审计与 stderr

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR6-EXIT-DRAIN",
  "family": "ocr-needs-exit-drain",
  "expected_files": [
    "docs/batches/PR6-EXIT-DRAIN.spec.md",
    "src-tauri/src/api_handlers/commands.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/api_server.rs"
  ],
  "max_lines_added": 183,
  "max_lines_removed": 58,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F163",
      "file": "src-tauri/src/api_handlers/commands.rs",
      "line": 168,
      "fix": "退出排空：RunningApi 暴露 active 计数，api_stop_for_exit 拒新后等归零（30s 默认+env 覆盖），超时 WARN 强退（拍板=30s 后强退）"
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
