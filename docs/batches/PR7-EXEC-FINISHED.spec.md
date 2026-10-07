# Batch Spec: PR7-EXEC-FINISHED

## 目的

OCR NEEDS-HUMAN F291：execute-task 收尾后迟到的 chat-open-session 会以围观
模式重开并重挂守卫，把已结束的会话永久拦输入。拍板：收尾登记成败（进程内
存活不清理）；迟到跳转成功=只读查看不挂守卫、失败=不自动跳转让用户决定
（失败任务不标记完成，是否重试由用户决定——已有行为：失败不改列，仅 trace/
会话落 failed）。

## 人类可读摘要

- family: ocr-needs-exec-finished
- 预估 diff: 4 files / +78/-7
- 测试：迟到双分支（失败不切 + 成功切过去看最终历史），先红后绿

## 红线

- 正常围观流（未收尾）行为零变化
- finished 表仅前端防复活用，不改后端任何语义

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR7-EXEC-FINISHED",
  "family": "ocr-needs-exec-finished",
  "expected_files": [
    "docs/batches/PR7-EXEC-FINISHED.spec.md",
    "src/components/ChatPanel.test.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/constants.ts"
  ],
  "max_lines_added": 128,
  "max_lines_removed": 57,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F291",
      "file": "src/components/ChatPanel/ChatPanel.tsx",
      "line": 783,
      "fix": "finishedExecTasks 收尾表：execute-task 收尾登记成败；迟到的 chat-open-session 成功=只读查看不挂守卫、失败=不自动跳转让用户决定（拍板）"
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
