# Batch Spec: W9-ASK

## 目的

用户需求:工作流跑偏防护——①拆解前 AI 可澄清(goal 级);②节点执行中可向用户提问(阻塞式,
带假设兜底);③问答走通知中心(后台可答,答完续跑);④所有用户答案/纠偏/原因沉淀为
**双层档案**(工作流决策摘要 + 卡片档案),注入后续节点执行,维持整体水平不跑偏。

设计依据:`docs/WORKFLOW-CLARIFY-AUDIT-DESIGN-2026-10-07.md` v2(拍板 1-8)。

## 人类可读摘要

- family: workflow-canvas
- 预估 diff: ~14 files / +1100/-60 lines
- OCR 计划: r1, timeout 2400s, comments ≤ 6(重点:忽略问题工作流也能走、注入有界、
  waiter 生命周期、档案 agent 不可自由写)

## 设计

1. **双层档案**(`db/brief.rs` 新模块):`brief_entries` 表(workflow_id / task_id NULL=全局 /
   kind qa|rework|manual|clarify / source user|agent|qa|system / text≤200 / reason≤80);
   单源 DDL + ensure(db/mod.rs);注入 `brief_for_injection` 全局最近 6 条≤1200 字 + 卡最近 8 条≤800 字;
   工作流删除级联清档案;**写入口全部代码路径,agent 无自由写档案工具**。
2. **执行提问 ask 端**(runner + 新 ask_user 工具,仅 workflow origin 注册):
   契约 assumption 必填(红线);预算 ≤2 问/节点(超预算返回"按假设继续"不落队列);
   提问模式=从不 → 直接返回假设;落 notifications(kind=workflow_question, id=wfq:{qid})+
   `question_waiters`(AppState,oneshot,照 confirm_requests 形态)等待;
   超时(默认 24h)/通道关闭 → 工具结果 = assumption + 卡备注"1 问未答,按假设执行" + 审计 Warn;
   run 收尾/停止 → 本 run pending 问题批量 dismissed。
3. **问答应答端**(`workflow_questions.rs`):`workflow_question_respond(question_id, action, answer?)`;
   action=answer/assume/dismiss;①resolve 通知 ②落档案(kind=qa,answer 原文或"未答按假设:{assumption}")
   ③waiter 存活 → take sender 发送(answer 原文;assume/dismiss → assumption)④审计 + notifications-changed;
   run 已死 → ②仍执行(重跑生效),③静默跳过。
4. **clarify 统一**(`workflow_clarify.rs` 新模块):`workflow_clarify(goal, attachments)` →
   `{questions≤3, attempts}`;契约:questions 空=信息足够/每问必带 default/why 一句/不问 goal 已写明/
   只问一轮;校验 `parse_and_validate_clarify` 超限**截断**不拒整包,default 缺失取 options[0],
   双缺丢弃该问;失败重试 1 次再失败返回空 questions(outcome=degraded 审计)——增强非闸门;
   附件抽取块从 decompose 提为 `pub(crate) build_attachment_blocks` 共享。
5. **decompose 扩展**:+`clarifications` 参数(prompt 三段:指引+澄清记录+契约;澄清答案落工作流档案
   kind=clarify);输出契约 +`assumptions`(≤5×≤60,缺失不拒);`workflows` 表 +`clarify_meta TEXT`
   (JSON `{granularity, answers[], extra}`,重拆预填)。
6. **前端**:WorkflowPage hero 状态机 idle→clarifying→clarify|decomposing→edit;
   `ClarifyCard.tsx`(四件套:问题/why/选项 chips+其他/用假设;已答折叠摘要;开始拆解永远可点,未答落 default);
   NotificationsPage 问题卡(选项 chips+输入框+按假设继续/忽略);types.ts 同步。

## 红线

- **忽略问题工作流也能走**:未答/超时/超预算/模式关闭 → 恒定返回 assumption,绝不无限阻塞
- 档案注入有界(1200/800 字封顶);agent 无自由写档案工具;条目只经代码路径
- clarify 校验超限截断不拒整包;失败降级直拆(增强非闸门);澄清答案 ≤200 字/题
- 问答生命周期:run 死后回答只落档案不唤醒;停止/删除批量失效 pending 问题
- 不动拓扑调度与 W-QA 语义;brief_entries/workflow_audit 表不做 runs 实体化

## 机器可读(脚本读取,勿改格式)

```json
{
  "batch_id": "W9-ASK",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W9-ASK.spec.md",
    "docs/WORKFLOW-CLARIFY-AUDIT-DESIGN-2026-10-07.md",
    "docs/rust-bot-architecture.md",
    "DEVLOG.md",
    "src-tauri/src/db/brief.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/workflow_clarify.rs",
    "src-tauri/src/workflow_questions.rs",
    "src-tauri/src/workflow_decompose.rs",
    "src-tauri/src/workflow_runner.rs",
    "src-tauri/src/notifications.rs",
    "src-tauri/src/app_state.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/lib.rs",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/ClarifyCard.tsx",
    "src/components/WorkflowCanvas/GoalNode.tsx",
    "src/components/NotificationsPage/NotificationsPage.tsx",
    "src/lib/workflowAsk.ts",
    "src/lib/notifications.ts",
    "src/types.ts"
  ],
  "max_lines_added": 1400,
  "max_lines_removed": 120,
  "findings": [
    {"id": "W9-1", "file": "src-tauri/src/db/brief.rs", "line": 1, "fix": "brief_entries 单源 DDL + ensure + insert(trim截断/上限) + 注入装配(全局6条1200字/卡8条800字) + 级联删除 + 单测"},
    {"id": "W9-2", "file": "src-tauri/src/db/mod.rs", "line": 175, "fix": "ensure_workflows_clarify_meta + ensure_brief_entries 挂入 open_db 迁移链"},
    {"id": "W9-3", "file": "src-tauri/src/workflow_clarify.rs", "line": 1, "fix": "clarify 契约段 + parse_and_validate_clarify(截断/default兜底/双缺丢弃) + command(重试1次/降级空questions) + 审计 + 单测"},
    {"id": "W9-4", "file": "src-tauri/src/workflow_decompose.rs", "line": 391, "fix": "附件抽取块提为 pub(crate) build_attachment_blocks;+clarifications 参数(prompt三段);契约+assumptions"},
    {"id": "W9-5", "file": "src-tauri/src/workflow_questions.rs", "line": 1, "fix": "workflow_question_respond(resolve通知→落档案→唤醒waiter→审计广播) + run级失效回收 helper + 单测"},
    {"id": "W9-6", "file": "src-tauri/src/app_state.rs", "line": 158, "fix": "question_waiters 注册表 + 访问器(照 confirm_requests 形态)"},
    {"id": "W9-7", "file": "src-tauri/src/workflow_runner.rs", "line": 579, "fix": "ask_user 工作流注册(assumption必填/预算≤2/模式开关) + oneshot等待(默认24h) + 超时按假设 + 收尾失效回收 + 档案注入 TaskExecCtx"},
    {"id": "W9-8", "file": "src-tauri/src/notifications.rs", "line": 29, "fix": "KIND_WORKFLOW_QUESTION 常量(队列复用 notifications 表)"},
    {"id": "W9-9", "file": "src-tauri/src/db/workflow.rs", "line": 35, "fix": "DDL+clarify_meta 幂等 ALTER + Workflow/SaveInput/Load 透传"},
    {"id": "W9-10", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 308, "fix": "hero 状态机 +clarifying/clarify 两态;runDecompose 先 clarify;ClarifyCard 装配;重拆预填 clarify_meta"},
    {"id": "W9-11", "file": "src/components/WorkflowCanvas/ClarifyCard.tsx", "line": 1, "fix": "澄清卡组组件(四件套/折叠摘要/开始拆解永远可点/未答落default/其他补充)"},
    {"id": "W9-12", "file": "src/components/NotificationsPage/NotificationsPage.tsx", "line": 1, "fix": "workflow_question 问题卡:选项chips+输入框+按假设继续/忽略 → workflow_question_respond"},
    {"id": "W9-13", "file": "src/types.ts", "line": 153, "fix": "ClarifyQuestion/ClarifyResult/WorkflowQuestionPayload/Workflow.clarifyMeta 类型同步"},
    {"id": "W9-14", "file": "src-tauri/src/bot/registry.rs", "line": 305, "fix": "SCHEMA_ASK_USER(assumption 进 required) + TOOLS_TABLE 条目 + call_ask_user(透传 ctx.stop) + 三处计数断言 37→38（spec 对齐：实现期扩散——工具注册走 registry 单源）"},
    {"id": "W9-15", "file": "src-tauri/src/bot_chat.rs", "line": 1395, "fix": "TaskExecCtx +brief/ask 字段 + render 注入档案 + run_task_in_chat_with 注册/注销 ask_contexts（成对无早退）（spec 对齐：会话生命周期只能在会话建立处接）"},
    {"id": "W9-16", "file": "src-tauri/src/db/tasks.rs", "line": 421, "fix": "delete_tasks 尾部接 brief_delete_task 卡层级联（尽力而为不炸主删除）（spec 对齐：OCR r1 critical 修复落点）"},
    {"id": "W9-17", "file": "src/components/WorkflowCanvas/GoalNode.tsx", "line": 14, "fix": "assumptions 折叠条（🤖 拆解假设 N，nm-card 同 report 条样式）（spec 对齐：assumptions 展示位在总目标卡）"},
    {"id": "W9-18", "file": "src/lib/workflowAsk.ts", "line": 1, "fix": "clarifyWorkflow/respondWorkflowQuestion + 三个类型（前端 lib 层，invoke 封装惯例）"}
  ],
  "assertions_min": {
    "src-tauri/src/workflow_clarify.rs": 6,
    "src-tauri/src/db/brief.rs": 5,
    "src-tauri/src/workflow_questions.rs": 4
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 2400,
    "expected_max_comments": 6
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
