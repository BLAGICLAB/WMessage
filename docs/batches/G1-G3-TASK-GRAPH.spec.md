# Batch Spec: G1-G3 任务图谱（合并批）

```json
{
  "batch_id": "G1-G3-TASK-GRAPH",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G1-G3-TASK-GRAPH.spec.md",
    "docs/TASK-GRAPH-DESIGN-2026-10-05.md",
    "docs/MANUAL-SMOKE-ACCEPTANCE-TASK-GRAPH-2026-10-05.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/people.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/profile.rs",
    "src-tauri/src/task_autotag.rs",
    "src-tauri/src/workflow_decompose.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/task_out.rs",
    "src-tauri/src/workflow_runner.rs",
    "src/App.tsx",
    "src/types.ts",
    "src/storage.ts",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/graph-build.ts",
    "src/components/GraphPage/physics.ts",
    "src/components/GraphPage/graph-build.test.ts",
    "src/components/GraphPage/physics.test.ts",
    "src/components/GraphPage/GraphPage.test.tsx",
    "package.json",
    "DEVLOG.md"
  ],
  "max_lines_added": 4000,
  "max_lines_removed": 100,
  "findings": [
    {"id": "G1-1", "file": "src-tauri/src/db/tasks.rs", "line": 1, "fix": "owner_id 列/迁移/upsert/导出 v2 信封/导入 v1-v2 归属归一 + 测试"},
    {"id": "G1-2", "file": "src-tauri/src/db/people.rs", "line": 1, "fix": "成员注册表（upsert 最新名/is_self 只升不降/占位兜底）+ people_list"},
    {"id": "G1-3", "file": "src-tauri/src/profile.rs", "line": 1, "fix": "personId 首访生成固化 + people 自我行 + 导出资料卡"},
    {"id": "G1-4", "file": "src/App.tsx", "line": 1, "fix": "visibleTasks owner 过滤 + 图谱导航 + 归档打标转变钩子"},
    {"id": "G2-1", "file": "src-tauri/src/task_autotag.rs", "line": 1, "fix": "归档打标守卫链/一次性推理/校验链/回写广播审计"},
    {"id": "G3-1", "file": "src/components/GraphPage/graph-build.ts", "line": 1, "fix": "建图纯函数（dep/member 边 + hub + 过滤器 + 聚合）"},
    {"id": "G3-2", "file": "src/components/GraphPage/physics.ts", "line": 1, "fix": "零依赖力导向物理（斥力/弹簧/向心/碰撞/预稳定）"},
    {"id": "G3-3", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 1, "fix": "Canvas 渲染 + 交互 + 脏检查节绘 + 同步首绘 + 双主题"}
  ],
  "assertions_min": {
    "src-tauri/src/db/tasks.rs": 8,
    "src-tauri/src/db/people.rs": 5,
    "src-tauri/src/task_autotag.rs": 5
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 2400,
    "expected_max_comments": 6
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```


## 目的

任务图谱模块三批合入（设计 docs/TASK-GRAPH-DESIGN-2026-10-05.md，老板拍板方案）：
多人任务卡汇总的归属基座（G1-OWNER）、归档自动打标（G2-AUTOTAG）、Obsidian 式
关系图谱（G3-GRAPH）。

## 人类可读摘要

- family: task-graph（三批同族：归属/打标/图谱共享 ownerId 数据模型）
- 预估 diff: 30 files / +3245/-46 lines（含新模块 db/people.rs、task_autotag.rs、
  src/components/GraphPage/* 四件、设计文档 + 冒烟清单）
- 验收: vitest 441 / cargo test 1334 全绿 + test-fast 全部门禁 + 浏览器 mock 渲染
  真实组件 GUI 冒烟（截图 gui-test-screenshots/graph-0*.png）+
  docs/MANUAL-SMOKE-ACCEPTANCE-TASK-GRAPH-2026-10-05.md 人工清单

## 方案要点（决策记录见设计文档 §0）

- **归属**：tasks.owner_id（幂等 ALTER，NULL 恒等于本人）；people 表（upsert 最新名
  覆盖 + is_self 只升不降 + 占位兜底）；profile.personId 首访生成 UUID 固化；
  导出 v2 信封 {version, exportedAt, profile, people, tasks}（NULL owner 盖章本人），
  导入 untagged 双格式（v1 裸数组视为本人，永久兼容）；归属归一 = 任务自带 ownerId
  优先否则信封 pid，等于自己归 NULL → 透明转发不串档。
- **看板隔离**（老板拍板）：App visibleTasks 与挂件选择器叠加 `!t.ownerId`，
  看板/归档/回收站/⌘K 默认只看自己；图谱/统计看全量。
- **打标**：归档转变（前端 prev/next 观测，覆盖 tick/reload/远端三路径）fire-and-forget
  task_autotag；守卫链（done ∧ archived ∧ 未软删 ∧ tags 空 ∧ owner NULL）幂等；
  summarize_messages 一次性推理（全局 active 模型，统一词表有意不用按卡覆盖）；
  校验链 ≤3 个 × ≤12 字截断不整包拒；apply_task_patch 持久化 + 事件广播 + 审计。
- **图谱**：graph-build 纯函数（dep 有向边 + member 边连工作流 hub；排除软删含归档；
  悬空引用丢边；统计口径按 completedAt 归年）；physics 零新依赖手写 d3-force 等效
  模型（网格短程斥力 + 弹簧 + 向心 + 碰撞，预稳定 150 tick 首帧即收敛）；
  GraphCanvas（Canvas 2D + hover 邻接高亮其余淡出 + 拖拽固定 + 指针锚缩放 +
  脏检查节绘 + 同步首绘抗 rAF/RO 节流 + CSS 变量双主题）；GraphPage（统计条/
  过滤器侧栏/详情面板——外来卡只读无「在看板打开」）。

## 红线

- origin（画布归属）语义不动；owner 与画布归属正交
- db_load 仍返全量（owner 过滤只在渲染层）；SQLite 事务边界/busy_timeout 纪律不动
- v1 导出文件永久可导；机器人/API/子 agent 建卡恒为本人（owner_id NULL）
- 不新增错误码；不触碰第三方导入/工作流执行路径
