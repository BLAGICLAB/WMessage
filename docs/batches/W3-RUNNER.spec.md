# Batch Spec: W3-RUNNER

## 目的

SPEC `docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md` 批次 W3：执行引擎——
workflow_run 拓扑调度（就绪即跑）+ 失败跳过传播（Airflow all_success）+ 断点续跑 +
workflow_stop（取消未启动队列）+ 系统通知 + TaskExecOrigin::Workflow 扩展 +
前端「开始执行/停止」点亮 + 进度显示。

## 人类可读摘要

- family: workflow-canvas
- 覆盖 findings: SPEC §8 执行引擎 / §12 通知 / §5.1 执行态（W3 子集）
- 预估 diff: ~8 files / +650/-25 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5（重点：调度竞态/取消语义/失败传播正确性）

## 红线

- family 一致：不碰 W2 拆解与 W4 导入导出
- 复用不另造：并发闸 = SubagentGate（wait_slot_cancellable）；执行 = run_task_in_chat
  （ExecGuard 防重入/30min 超时/预算/会话即记录全部继承）；通知 = 调度器 notify 同款；
  广播 = broadcast_after_mutation；⏭ 跳过写 note = 调度器 ⏰ 摘要前置同款 RMW 合并
- run_task_in_chat 主链路**零修改**（只加 TaskExecOrigin::Workflow 枚举值 + 前缀）
- 停止语义（设计 §8.1）：workflow_stop 取消**未启动**队列（含在 Gate 排队的）；
  运行中会话走既有 StopGuard /stop 通道，本批不做按会话远程停
- 断点续跑（设计 §8.4）：workflow_run 时 done+success 节点直接视为已完成（解锁下游）；
  成功判定 = column=done 且 result.status ∈ {缺失, success}（与节点描边判定一致）

## 调度模型（就绪即跑，非整层屏障）

```
workflow_run(wfId)：防重入（RUNS 注册表）→ 建图（环/悬空防御）→ 统计
  → spawn 控制器：入度 0 就绪集 spawn 节点任务（每任务一个 tokio task）
     节点任务：wait_slot_cancellable(闸) → run_task_in_chat(origin=Workflow)
             → 终态判定 → mpsc 上报
     控制器：成功 → 下游入度减一归零即 spawn；失败 → 传递闭包全标 ⏭ 跳过；
             取消 → 未解析节点全标 ⏭ 已停止；全部终态 → 通知 + 审计 + 注销
```

## spec 起草后自查三条（APW-02a）

1. expected_files：新增 workflow_runner.rs 需登记 rust-bot-architecture.md（模块地图门禁）
2. budget B 类：runner ~420 行（含单测）+ 前端 ~120 行
3. bot_artifacts::should_emit 的 TaskExecOrigin 穷尽匹配需加 Workflow 臂
   （产物登记语义 = Scheduled 同款无人值守）

## 自主执行规则

spec 起草即视为 reviewer 批准（自主模式），agent 全权执行至 commit。
完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + follow-up。

## Stop 条件（触发即停，报 reviewer）

- compile_failure / architecture_blocker / family_heterogeneity /
  new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W3-RUNNER",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W3-RUNNER.spec.md",
    "docs/rust-bot-architecture.md",
    "src/components/WorkflowCanvas/GoalNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/bot_artifacts.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/workflow_runner.rs"
  ],
  "max_lines_added": 800,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W3-1", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "新模块：建图/就绪即跑/跳过传播/断点续跑/取消/通知 + 单测"},
    {"id": "W3-2", "file": "src-tauri/src/bot_chat.rs", "line": 1278, "fix": "TaskExecOrigin::Workflow + 🔀 前缀"},
    {"id": "W3-3", "file": "src-tauri/src/bot_artifacts.rs", "line": 90, "fix": "origin 匹配臂加 Workflow（=Scheduled 语义）"},
    {"id": "W3-4", "file": "src-tauri/src/lib.rs", "line": 1, "fix": "注册 workflow_run/stop/is_running 三命令 + 模块"},
    {"id": "W3-5", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "开始执行/停止/进度点亮"},
    {"id": "W3-6", "file": "src/components/WorkflowCanvas/GoalNode.tsx", "line": 1, "fix": "总目标卡进度 done/total"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_runner.rs": 8
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
