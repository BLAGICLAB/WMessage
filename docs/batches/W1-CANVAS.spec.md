# Batch Spec: W1-CANVAS

## 目的

工作流画布骨架（SPEC `docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md` 批次 W1）：导航栏「工作流」栏目 +
React Flow 画布 + 手动加/删卡连线（环校验）+ workflows 表 + Task 四字段 + 草稿态/指纹 diff 保存。
不含 AI 拆解（W2）、执行引擎（W3）、导入导出（W4）。

## 人类可读摘要

- family: workflow-canvas
- 覆盖 findings: SPEC §3 数据模型 / §5 模块 UI / §7 保存 / §3.3 过滤（W1 子集）
- 预估 diff: ~16 files / +1600/-60 lines
- OCR 计划: r2, timeout 1800s, 期望 comments ≤ 5（重点：指纹 diff 事务、环检测、字段 ripple）

## 红线

- family 一致性：本批只含 workflow-canvas，不混入其他 family
- 看板/归档/回收站/挂件/聊天对 origin=user 任务零行为变化（工作流卡默认过滤，开关默认关）
- 4 处同步点全过：TS Task / Rust Task / DB 列 / 保存路径校验
- Task 四字段全 Option + serde default：老库、老事件、老导出文件反序列化不受影响

## spec 起草后自查三条（APW-02a）

1. expected_files 覆盖全部写入路径：Task struct 字面量 ripple 8 处
   （tasks.rs:398 / bot/tools.rs:404,1580 / bot_orchestrator.rs:144,1807 / task_out.rs:34 /
   api_handlers/mod.rs:68 / api_handlers/handlers.rs:303 / db/mod.rs:608）+ 测试文件
2. budget 是 B 类（新模块多文件，按文件分别估）
3. 新增字段逐条列出 patch 白名单与校验规则（见下）

### 新增字段契约

| 字段 | TS | Rust | DB | 校验 |
|---|---|---|---|---|
| origin | `"user"\|"workflow"?` | `Option<String>` | `origin TEXT`（缺省 'user' 语义 = NULL 视为 user） | patch 白名单限 user/workflow |
| workflowId | `string?` | `Option<String>` | `workflow_id TEXT` | patch 拒空串 |
| dependsOn | `string[]?` | `Option<Vec<String>>` | `depends_on TEXT`(JSON) | 元素非空；保存路径另查环/悬空 |
| canvasPos | `{x,y}?` | `Option<CanvasPos>` | `canvas_x REAL`+`canvas_y REAL` | x/y 为有限数 |

### 指纹 diff 保存（workflow_save 锁内事务）

- fp(node) = 递归内容指纹 `(title|note|tags|sorted fp(upstream))`，memo DFS，环即报错
- fp 相同 → 保留原任务 id（连带 column/result/budget/子任务），仅更新 canvasPos（RMW 基线锁内现读）
- fp 不同/新增 → 建新卡（新 uuid，origin=workflow）；消失者 → 删卡
- 校验：nodes ≤ 30、title 非空 ≤80、note ≤500、name ≤80、goal ≤500、dependsOn 引用存在/无自环

## 自主执行规则

spec 起草即视为 reviewer 批准（自主模式），agent 全权执行至 commit，不中途报 status / exit / diff。
完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + follow-up。
过程证据落 ~/.openclaw/cache/W1-CANVAS/。

## Stop 条件（触发即停，报 reviewer）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W1-CANVAS",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-CODE-REVIEW-2026-10-04-w1.json",
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md",
    "docs/batches/W1-CANVAS.spec.md",
    "docs/rust-bot-architecture.md",
    "package.json",
    "package-lock.json",
    "src/types.ts",
    "src/App.tsx",
    "src/lib/workflowVisibility.ts",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/TaskNode.tsx",
    "src/components/WorkflowCanvas/GoalNode.tsx",
    "src/components/WorkflowCanvas/graph.ts",
    "src/components/WorkflowCanvas/graph.test.ts",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/task_out.rs",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_orchestrator.rs"
  ],
  "max_lines_added": 2200,
  "max_lines_removed": 120,
  "findings": [
    {"id": "W1-1", "file": "src/types.ts", "line": 58, "fix": "Task 加 origin/workflowId/dependsOn/canvasPos 四字段"},
    {"id": "W1-2", "file": "src-tauri/src/db/tasks.rs", "line": 90, "fix": "Rust Task 镜像四字段 + upsert SQL + task_from_row + patch 白名单"},
    {"id": "W1-3", "file": "src-tauri/src/db/mod.rs", "line": 106, "fix": "workflows DDL + tasks 五列幂等 ALTER"},
    {"id": "W1-4", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "新模块：Workflow CRUD + 指纹 diff 保存 + 级联删除 + 单测"},
    {"id": "W1-5", "file": "src-tauri/src/lib.rs", "line": 472, "fix": "注册 workflow_save/load/list/rename/delete 五命令"},
    {"id": "W1-6", "file": "src/App.tsx", "line": 62, "fix": "RailView+NAV_ITEMS 加 workflow；可见性过滤选择器"},
    {"id": "W1-7", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "画布页：草稿态/加删卡/连线校验/保存（含 TaskNode/GoalNode/graph.ts 同目录新文件）"},
    {"id": "W1-8", "file": "src/components/WidgetApp/WidgetApp.tsx", "line": 638, "fix": "挂件可见列表过滤 origin=workflow"},
    {"id": "W1-9", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 193, "fix": "工作流分区：显示工作流任务开关"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workflow.rs": 8
  },
  "ocr_plan": {
    "rounds": 2,
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

## 验证命令

```
npx tsc --noEmit
npx oxlint src
npx vitest --run
cargo test  (src-tauri)
cargo clippy (src-tauri)
python3 scripts/batch-verify.py docs/batches/W1-CANVAS.spec.md
```
