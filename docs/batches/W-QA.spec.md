# Batch Spec: W-QA 工作流质量优化——结构化交接 + 验收标准 + 证据结果 + 有界重试/返工环

```json
{
  "batch_id": "W-QA",
  "family": "feat",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/W-QA.spec.md",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot_scheduler.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/exec_steps.rs",
    "src-tauri/src/prompts/execute.rs",
    "src-tauri/src/task_out.rs",
    "src-tauri/src/workflow_decompose.rs",
    "src-tauri/src/workflow_runner.rs",
    "src-tauri/tests/exec_trace.rs",
    "src-tauri/tests/llm_integration.rs",
    "src-tauri/tests/task_chat_exec.rs",
    "src/components/TaskCardContent.tsx",
    "src/components/WorkflowCanvas/GoalNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/graph.ts",
    "src/lib/workflowPrompt.ts",
    "src/types.ts"
  ],
  "max_lines_added": 1500,
  "max_lines_removed": 200,
  "max_new_files_lines": 120,
  "findings": [
    {"id": "WQ-1", "file": "src-tauri/src/prompts/execute.rs", "line": 16, "fix": "EXECUTE 提示词规则 4 补 🔀 工作流分支（必须调 complete_task，节点成功判定依赖 column=done，原口径缺失会把做完的节点误判失败并向下游传播跳过）+ 带验收标准时逐条对照自检"},
    {"id": "WQ-2", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "A2 证据结果：每节点收尾 RMW 写结构化 result（status=success|failed|incomplete + summary≤300 字 + error + artifacts + attempt，build_node_result 纯函数）；B2 上游简报：depends_on 反转建直接上游表，spawn 前装配（单上游 ≤600 字总 ≤2400 字）；C1 失败自动重试一次（should_retry，熔断除外，3s 退避）；C2 rubric 评审（JSON verdict/overall/issues，解析失败降级纯文本）落 workflows.last_report + workflow-report 事件；C3 有界返工环（needsRework 节点含传递下游返工一轮，Done 卡先重置，返工集内按依赖序调度，终审只覆盖返工节点；每节点至多 1 次返工、每次运行至多 2 次评审）"},
    {"id": "WQ-3", "file": "src-tauri/src/bot_chat.rs", "line": 1, "fix": "B1 上下文注入：新增 TaskExecCtx{goal, upstream_brief} + render()；run_task_in_chat_ctx 新入口（旧签名委托 None，手动/定时/批量零改动）；build_task_block 追加【工作流总目标】【上游产出】段 + 验收标准行，会话落库与执行消息同源"},
    {"id": "WQ-4", "file": "src-tauri/src/db/tasks.rs", "line": 29, "fix": "B3 卡即契约：tasks 表新增 acceptance 列（DDL + 幂等 ALTER 迁移 + Task 字段 serde default + 读写列清单）；各 Task 测试字面量补字段"},
    {"id": "WQ-5", "file": "src-tauri/src/db/workflow.rs", "line": 109, "fix": "acceptance 进 WorkflowNodeDraft/validate_nodes（≤120 字）/新卡构造/内容指纹（改验收=换新卡与 note 同语义）；workflows 表新增 last_report/last_report_at 列（ensure + 定点写 workflow_set_report，画布保存不冲掉）；模板 WorkflowFileNode 透传 acceptance"},
    {"id": "WQ-6", "file": "src-tauri/src/workflow_decompose.rs", "line": 23, "fix": "B4 拆解契约：JSON 契约段加 acceptance 字段（≤120 字缺失容忍）+ 交接边界显式化（note 必须写具体产物文件名，下游按名引用）；DEFAULT_DECOMPOSE_GUIDANCE 同步；校验链 trim/上限"},
    {"id": "WQ-7", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "保存载荷携带 acceptance（与 model 同款兜底）；workflow-report 事件监听（activeIdRef 防过期闭包）+ lastReport 恢复（损坏降级 null）；GoalNode data 传 report"},
    {"id": "WQ-8", "file": "src/components/WorkflowCanvas/GoalNode.tsx", "line": 1, "fix": "总目标卡新增收尾审校报告折叠展示（verdict 徽标 pass/partial/fail + issues 列表 + 返工终审标注）"},
    {"id": "WQ-9", "file": "src/components/TaskCardContent.tsx", "line": 147, "fix": "卡片验收标准只读展示（📌 验收：…，画布/看板/挂件共用）"},
    {"id": "WQ-10", "file": "src/types.ts", "line": 132, "fix": "Task.acceptance / Workflow.lastReport/lastReportAt / WorkflowReport 类型（与 Rust camelCase 对齐）；workflowPrompt.ts 默认指引与 Rust 侧同步"}
  ],
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 5
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

工作流引擎（W3/W4/W5）已有 DAG 调度、失败传播、断点续跑，但卡片执行是「信息孤岛」：
执行时不注入总目标与上游产出、完成全靠模型自律（无证据、EXECUTE 提示词工作流口径
缺失会把做完的节点误判失败连锁跳过）、失败无重试、收尾无审校。本批吸收业界模式收口：
typed-schema handoff（结构化交接 + 压缩摘要，Anthropic 多 agent 实战教训）、
rubric 评审（LLM-as-Judge，evaluator-optimizer）、有界 Reflexion 返工环、
有界重试 + 退避（durable execution）。新增 tasks.acceptance 列（卡即契约：拆解生成
一行可验证完成标准，执行注入自检、画布/看板只读展示）与 workflows.last_report 列
（评审报告持久化 + 画布总目标卡展示）。手动/定时/批量执行链路零行为变化（ctx 传 None）。
