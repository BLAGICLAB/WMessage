# Batch Spec: W6-MODEL

## 目的

每张工作流节点卡可指定执行用大模型（编程卡用 GLM、绘图类卡指向图像网关等）——
模型条目复用设置页「模型设置」的模型库（ModelEntry：base_url/model/vendor 完整连接信息），
卡上存条目 id；不指定 = 跟随全局 active 模型。

## 人类可读摘要

- family: workflow-canvas
- 预估 diff: ~9 files / +330/-30 lines
- OCR 计划: r1, timeout 1800s, comments ≤ 5（重点：override 解析链 key 正确性、老路径零影响）

## 设计

1. **Task.model**（Option<String> = ModelEntry.id）四同步点：TS / Rust struct /
   DB 列 `model TEXT`（幂等 ALTER）/ task_patch 白名单
2. **解析**（纯函数 `resolve_model_override(cfg, entry_id)` 单测锚点）：
   models_by_provider 两协议列表合并找 entry（enabled 校验）→
   (base_url, model, api_provider, api_key=厂商 key→全局 key 兜底)；找不到/禁用 → 报错可读
3. **执行链**：workflow_runner 读 task.model → run_task_in_chat 新参
   `model: Option<String>` → run_model_loop 新参 `model_override: Option<String>` →
   薄壳解析覆盖 http cfg 三元组与 key；既有调用方全部传 None（行为零变化）
4. **保存/文件**：WorkflowNodeDraft.model + workflows 节点卡 passthrough（保留卡不动、
   新卡 None）；.wflow.json 节点可选 `model`（本地字段，导入透传）
5. **前端**：节点卡（已保存）加模型下拉——选项 = 模型库全条目 + 「跟随全局」；
   选择即 task_patch；未保存卡不显示下拉

## 红线

- 不动子 agent model_profile 机制（独立通道）
- 全局 active 路径字节级不变（override=None 时解析链短路）
- key 永不入 Task/文件（只存 entry id，key 按厂商实时读凭据存储）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W6-MODEL",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W6-MODEL.spec.md",
    "src/components/WorkflowCanvas/TaskNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/types.ts",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot_scheduler.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/exec_steps.rs",
    "src-tauri/src/task_out.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/workflow_runner.rs"
  ],
  "max_lines_added": 480,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W6-1", "file": "src/types.ts", "line": 58, "fix": "Task.model 字段"},
    {"id": "W6-2", "file": "src-tauri/src/db/tasks.rs", "line": 1, "fix": "Rust 字段+DDL+行解析+upsert+patch 白名单"},
    {"id": "W6-3", "file": "src-tauri/src/bot/config/types.rs", "line": 1, "fix": "resolve_model_override 纯函数+单测"},
    {"id": "W6-4", "file": "src-tauri/src/bot_model_loop.rs", "line": 1, "fix": "run_model_loop model_override 参数+薄壳解析"},
    {"id": "W6-5", "file": "src-tauri/src/bot_chat.rs", "line": 1422, "fix": "run_task_in_chat model 参数透传"},
    {"id": "W6-6", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "runner 读 task.model 传入"},
    {"id": "W6-7", "file": "src-tauri/src/db/workflow.rs", "line": 1, "fix": "草稿/文件格式 model 透传"},
    {"id": "W6-8", "file": "src/components/WorkflowCanvas/TaskNode.tsx", "line": 1, "fix": "节点模型下拉（WorkflowPage 装载模型库）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/config/types.rs": 4
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
