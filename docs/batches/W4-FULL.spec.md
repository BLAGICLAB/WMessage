# Batch Spec: W4-FULL

## 目的

W1–W4 全量对照（SPEC §14.0.5 收口条件）处置批：4f371cd..HEAD 合并 diff 复审
（73 条 = 1C+4H+37M+31L，报告本地 `docs/OCR-CODE-REVIEW-2026-10-04-w1-w4-full.json`）。
**1C+4H 全部随批修复**，68 条 medium/low 按主题三分落账
（OCR-FOLLOWUPS-INDEX.md「W1–W4 全量对照」节 FULL-1..68）。

## 修复清单（已修）

- **CRITICAL**：workflow_run 防重入 TOCTOU——contains_key 检查与 insert 之间隔着
  await（load_workflow_tasks），并发调用可双注册双跑 → check+insert 同锁原子完成，
  占坑后一切失败走 cleanup 出坑
- **HIGH（实为死锁）**：断点续跑必挂——done+success 节点计入 dag.nodes（total）但
  永不上报终态，控制器 `resolved.len() < total` 永真，recv 永久等待 →
  nodes 只含将执行节点 + 回归测试 `dag_nodes_exclude_done_success`
- **HIGH**：workflows 行 upsert 在事务外先落 → 移入 tx 同一事务（消孤儿行）
- **HIGH**：openWorkflow 不立即复位 running → A 在跑切 B 时停止按钮跨工作流残留 →
  查询前置 false（查询取真兜底不变）
- **HIGH**：TaskNode 草稿输入每次击键全量替换 nodes 数组 → 确认为 30 节点上限下
  无观察开销，随 W1F-6 已账（不修）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W4-FULL",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W4-FULL.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/workflow_runner.rs"
  ],
  "max_lines_added": 200,
  "max_lines_removed": 60,
  "findings": [
    {"id": "WFULL-1", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "TOCTOU 原子化 + nodes 排除 done+success（死锁）+ 回归测试"},
    {"id": "WFULL-2", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "workflows 行入事务"},
    {"id": "WFULL-3", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "openWorkflow 复位 running"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_runner.rs": 8
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
