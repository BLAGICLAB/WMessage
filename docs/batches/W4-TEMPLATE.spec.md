# Batch Spec: W4-TEMPLATE

## 目的

SPEC `docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md` 批次 W4（收官）：`.wflow.json`
模板导入导出（**实例化语义**：全新 workflow 行 + 全新任务 id，与 tasks_import 的
按 id 合并刻意分离）+ 工具栏导入/导出点亮 + GUI 冒烟验收清单文档。

## 人类可读摘要

- family: workflow-canvas
- 覆盖 findings: SPEC §4 文件格式 v1 / §13 W4 行
- 预估 diff: ~5 files / +420/-20 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5（重点：导入校验链完整性、路径处理、实例化无合并旁路）

## 红线

- family 一致：不碰 W1–W3 已落地行为
- **导入 = 实例化**：复用 `workflow_save_locked`（workflow_id=None → 新行 + 新 uuid），
  环/悬空/长度/重名等图规则全部由既有校验链承接，不新写第二套图校验
- 文件格式 v1（设计 §4）：`version` 必须 =1；节点 id 为文件内本地 id；`pos` 可缺省
  （缺省 → dagre 布局，与 AI 拆解同款）；上限 30 节点
- 导入读文件 1MB 硬顶（bounded read，同 tasks_import 的 check-then-act 防御思路）；
  导出走 `check_export_path`（.json）+ `atomic_write`
- 产物：导入/导出均写 bot.log 审计（workflow_import / workflow_export）

## 文件格式（v1，与设计 §4 一致）

```
{
  "version": 1, "generator": "wmessage 0.1.0", "exportedAt": "RFC3339",
  "name": "…", "goal": "…",
  "nodes": [ { "id": "n1", "title": "…", "note": "…", "tags": [],
               "dependsOn": ["n1"], "pos": [240, 80] } ]
}
```

- 导出节点按**拓扑序**排列（上游在前，可读性），dependsOn 用映射后的本地 id
- 导入：version≠1 / 节点空或 >30 / id 重复或为空 → 整包拒绝；图规则（环/悬空）
  由 save 链拒绝；dependsOn 重复元素自动去重；goal 缺省回退 name

## spec 起草后自查三条（APW-02a）

1. 无新增源文件（文件 IO 放 db/workflow.rs）→ 模块地图零变更
2. budget B 类：workflow.rs +260 行（含单测）+ 前端 +60 行
3. 广播走 WorkflowSaveOutcome（W3 修过的锁内载荷模式），不锁外重读

## 自主执行规则

spec 起草即视为 reviewer 批准（自主模式），agent 全权执行至 commit。
完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + follow-up。

## Stop 条件（触发即停，报 reviewer）

- compile_failure / architecture_blocker / family_heterogeneity /
  new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W4-TEMPLATE",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/MANUAL-SMOKE-ACCEPTANCE-WORKFLOW-2026-10-04.md",
    "docs/batches/W4-TEMPLATE.spec.md",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/lib.rs"
  ],
  "max_lines_added": 560,
  "max_lines_removed": 30,
  "findings": [
    {"id": "W4-1", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "文件格式 v1 结构体 + parse_workflow_file + 拓扑导出序 + workflow_export/import 命令 + 审计 + 单测"},
    {"id": "W4-2", "file": "src-tauri/src/lib.rs", "line": 1, "fix": "注册 workflow_export/workflow_import"},
    {"id": "W4-3", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "导入/导出点亮（dialog + 导入后打开新实例）"},
    {"id": "W4-4", "file": "docs/MANUAL-SMOKE-ACCEPTANCE-WORKFLOW-2026-10-04.md", "line": 1, "fix": "GUI 冒烟验收清单（SPEC §14.A–F ⭐ 子集）"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workflow.rs": 26
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
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
