# Batch Spec: W5-FUSE-r1

## 目的

W5-FUSE 的 OCR r1 comments 处置批：1H + 4M + 5L 随批修复，3 条缓期落账
（OCR-FOLLOWUPS-INDEX.md W5F-1..3）。父批 spec：`docs/batches/W5-FUSE.spec.md`
（首提交 `fcf4aaa` 已过全套门禁）。

## 修复清单（已修）

- **HIGH**：runButtonTitle 扩展三参（dirty/hasActive/doneCount）——dirty 的
  「先保存」提示不再被续跑分支吞掉；nodes 空但 doneCount>0 的 title 错配一并消除
- **medium**：doneCount 提为单一数据源（按钮态 + 总目标卡进度共用，入 memo deps）；
  熔断标记抽 `FUSE_MARKER` 共享常量（runner 识别与 fuse_message 同源防措辞漂移）；
  熔断上限输入改本地草稿 blur 提交（去每击键 bot_set_config IPC+落盘）；
  后端派生钳 1..=500（防巨值实质关闭熔断）
- **low**：陈旧注释（其余 50→100 ×2）、测试名 51st→cap_boundary、死变量删除、
  边界断言补默认 100（100 放行/101 熔断）
- **缓期 3 条** → W5F-1..3（全局入口位置=产品决策；NodeOutcome enum 化随下批；
  fuse_cap_override 测试通道保留）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W5-FUSE-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W5-FUSE-r1.spec.md",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/workflow_runner.rs"
  ],
  "max_lines_added": 120,
  "max_lines_removed": 50,
  "findings": [
    {"id": "W5R1-1", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "runButtonTitle 三参 + doneCount 单一源"},
    {"id": "W5R1-2", "file": "src-tauri/src/bot_model_loop.rs", "line": 1, "fix": "FUSE_MARKER + 钳 500 + 注释/测试清理"},
    {"id": "W5R1-3", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "熔断识别走共享常量"},
    {"id": "W5R1-4", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1, "fix": "上限输入 blur 提交"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_model_loop.rs": 8
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 1800,
    "expected_max_comments": 0
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```
