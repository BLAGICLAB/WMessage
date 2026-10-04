# Batch Spec: W4-TEMPLATE-r1

## 目的

W4-TEMPLATE 的 OCR r1 comments 处置批：1H + 4M + 4L 随批修复，6 条缓期入账
（OCR-FOLLOWUPS-INDEX.md W4F-1..6）。父批 spec：`docs/batches/W4-TEMPLATE.spec.md`
（首提交 `e9c1abb` 已过全套门禁）。

## 修复清单（已修）

- **HIGH**：导入成功 alert 先于 openWorkflow——失败时出现"已成功 + 重试又导入一份"
  悖论 → 先打开新实例再提示；列表刷新失败走 silent 弹窗既有模式
- **medium**：workflow_run 晚到响应 vs 新建/切换画布 → startRun 捕获 openSeq 快照，
  成功后 seq 比对才置 running
- **medium**：workflow_file_from 整卡深拷贝喂 topo → topo_export_order 改吃
  (id, deps) 视图（测试同步）
- **medium**：createBlank 丢 setRunning(false)（W4 实施时的回归）→ 补回 + 复位理由注释
- **low**：workflow_export 审计补 path（与 import 对等）；两处冗余 path.clone() 改 move；
  roundtrip 测试补 pos 透传断言（原"打乱坐标"注释与行为不符）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W4-TEMPLATE-r1",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W4-TEMPLATE-r1.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/db/workflow.rs"
  ],
  "max_lines_added": 140,
  "max_lines_removed": 80,
  "findings": [
    {"id": "W4R1-1", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "导入先开再提示 + startRun seq 守卫 + createBlank 复位注释"},
    {"id": "W4R1-2", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "topo (id,deps) 视图去深拷贝 + 审计 path 对等 + clone 清理 + 测试断言修正"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workflow.rs": 26
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
