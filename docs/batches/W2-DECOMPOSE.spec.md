# Batch Spec: W2-DECOMPOSE

## 目的

SPEC `docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md` 批次 W2：AI 拆解——
goal（自然语言总目标）→ 一次性 LLM 结构化输出 → 校验链 → dagre 自动布局 → 画布草稿。
点亮「AI 生成」与「重新生成」；设置页「工作流」分区加拆解提示词（指引段）编辑 + 恢复默认。

## 人类可读摘要

- family: workflow-canvas
- 覆盖 findings: SPEC §6 AI 拆解 / §9 布局 / §10 设置页（W2 子集）/ §5.3 重新生成
- 预估 diff: ~9 files / +700/-30 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5（重点：提示词两段式隔离、校验链完整性、一次性调用不写会话）

## 红线

- family 一致：本批只含 workflow-decompose，不碰执行引擎（W3）与导入导出（W4）
- 拆解是**一次性调用**：不开会话、不写 bot_messages、不进聊天记录（复用
  `bot_chat::summarize_messages` 的配置/密钥/客户端样板）
- 提示词两段式：用户只可编辑「指引段」（存 localStorage，随 invoke 上行）；
  「输出契约段」由代码硬拼追加——用户改指引段破坏不了 JSON 契约
- 校验链在服务端：fences 剥离 → JSON 解析 → 数量/长度/前向引用/去重后缀全过才入库草稿；
  解析失败重试 1 次（附错误反馈），再失败报错
- dependsOn 用数组下标引用且**必须 < 自身下标**（结构上杜绝环，服务端仍防御性校验）

## spec 起草后自查三条（APW-02a）

1. expected_files 覆盖：lib.rs 注册 + 模块地图登记（rust-bot-architecture.md，audit_module_map 门禁）
2. budget B 类：新模块 workflow_decompose.rs ~260 行 + 前端 ~300 行
3. 新增命令 workflow_decompose 无新增 DB 列/表（workflows/tasks 复用 W1 schema）

## 输出契约（代码硬拼段，模型可见、用户不可改）

```
{"subtasks":[{"title":"≤80字祈使句","note":"做什么/产出什么（≤500字）","dependsOn":[0]}]}
```

- subtasks 1~20 个（服务端硬顶 30 与 W1 保存上限对齐，契约要求 20 留余量）
- dependsOn 元素 = 数组下标，必须 < 自身下标；无依赖为 []
- 只输出 JSON，无围栏无解释（解析端同时容忍围栏与裸数组）

## 里程碑内验收锚点

- 拆解失败（模型/网络/校验）→ 可读报错，无部分入库（草稿只在内存）
- 拆解成功 → dagre 分层布局，无坐标字段由 LLM 产生（契约禁止）
- bot.log 审计事件 `workflow_decompose`（subtask 数 + 重试与否）

## 自主执行规则

spec 起草即视为 reviewer 批准（自主模式），agent 全权执行至 commit，不中途报 status。
完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + follow-up。

## Stop 条件（触发即停，报 reviewer）

- compile_failure / architecture_blocker / family_heterogeneity /
  new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W2-DECOMPOSE",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W2-DECOMPOSE.spec.md",
    "docs/rust-bot-architecture.md",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/graph.test.ts",
    "src/components/WorkflowCanvas/graph.ts",
    "src/lib/workflowPrompt.ts",
    "src-tauri/src/lib.rs",
    "src-tauri/src/workflow_decompose.rs"
  ],
  "max_lines_added": 900,
  "max_lines_removed": 60,
  "findings": [
    {"id": "W2-1", "file": "src-tauri/src/workflow_decompose.rs", "line": 1, "fix": "新模块：两段式提示词 + 一次性调用 + 校验链 + 重试 + 审计 + 单测"},
    {"id": "W2-2", "file": "src-tauri/src/lib.rs", "line": 1, "fix": "注册 workflow_decompose 命令"},
    {"id": "W2-3", "file": "src/lib/workflowPrompt.ts", "line": 1, "fix": "默认指引段 + localStorage 读写"},
    {"id": "W2-4", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1, "fix": "工作流分区加提示词编辑 + 恢复默认"},
    {"id": "W2-5", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "AI 生成/重新生成点亮 + loading/取消 + 竞态守卫"},
    {"id": "W2-6", "file": "src/components/WorkflowCanvas/graph.ts", "line": 1, "fix": "draftFromDecompose：下标依赖 → localId 映射 + dagre 布局"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_decompose.rs": 8
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
