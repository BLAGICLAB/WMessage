# Batch Spec: W7-TOPO

## 目的

用户实测（2026-10-04 23:36 bot.log）：拆解两次尝试均因 `subtasks=#4 -> 4`（第 5 卡
自引用）被整包拒绝。自引用/顺序写反是小模型高频手误且**可自动归一**，不应拒绝——
`validate_decompose` 升级为「归一代替硬拒」：自环剥离、任意下标 DAG 接受、
Kahn 拓扑重排重映射；仅真环拒绝且报出环内任务名。

## 修复设计

- 依赖归一两段式：去重 + 剥自环；越界引用仍拒绝（读不出意图）
- Kahn 环检测（同就绪层按原下标稳定）；成环 → Err 报环内任务名
- 无环 → 拓扑重排 + 旧→新下标重映射（重排后必然满足"下标 < 自身"，
  与下游 draftFromDecompose 的下标映射天然兼容）
- 契约段文案降硬为软（"尽量引用前面的任务，顺序写反系统会自动纠正"）
- 测试重写：前向引用接受+重排断言、自环剥离断言、真环报名断言、
  同就绪层稳定序；删除旧"前向引用拒绝"测试

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W7-TOPO",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W7-TOPO.spec.md",
    "src-tauri/src/workflow_decompose.rs"
  ],
  "max_lines_added": 160,
  "max_lines_removed": 60,
  "findings": [
    {"id": "W7-1", "file": "src-tauri/src/workflow_decompose.rs", "line": 1, "fix": "依赖归一（自环剥离/拓扑重排/真环报名）+ 测试重写"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_decompose.rs": 8
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```
