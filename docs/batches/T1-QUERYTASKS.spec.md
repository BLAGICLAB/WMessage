# Batch Spec: T1-QUERYTASKS 任务工具对齐数据模型——list/search 合并 + edit_task 新字段 + subtaskId

```json
{
  "batch_id": "T1-QUERYTASKS",
  "family": "bot-tools",
  "expected_files": [
    "docs/batches/T1-QUERYTASKS.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_skills/runtime.rs",
    "src-tauri/src/tool_guard.rs",
    "src-tauri/src/prompts/system.rs",
    "src-tauri/src/prompts/mod.rs",
    "src-tauri/tests/fixtures/tools_baseline.json",
    "src-tauri/tests/llm_integration.rs",
    "tests-audit/audit_bot_tools_alignment.py",
    "tests-audit/regen_tools_baseline.py",
    "docs/MANUAL-SMOKE-ACCEPTANCE-BOT-TOOLS-2026-10-05.md"
  ],
  "max_lines_added": 480,
  "max_lines_removed": 260,
  "findings": [
    {"id": "T1-1", "file": "src-tauri/src/bot/registry.rs", "line": 104, "fix": "list_tasks+search_tasks 合并为 query_tasks（query?/view/tag/limit；无 query=active 清单、有 query=all 全库）；schema/适配器/TOOLS_TABLE/registry_tests（33 表、31 主可见）同批"},
    {"id": "T1-2", "file": "src-tauri/src/bot/registry.rs", "line": 121, "fix": "edit_task 扩 model（空串=清除恢复跟随全局）/owner（成员名或 personId，空串=本人）两参；toggle/remove_subtask 扩 subtaskId（精确优先，回落文本）"},
    {"id": "T1-3", "file": "src-tauri/src/bot/tools.rs", "line": 173, "fix": "tool_query_tasks + 纯函数族（TaskView/parse_view/view_keep/keyword_hit/tag_keep/parse_limit/workflow_name_map/humanize_schedule/render_task_line），输出行带（工作流：名称）标记"},
    {"id": "T1-4", "file": "src-tauri/src/bot/tools.rs", "line": 625, "fix": "tool_edit_task 接 model/owner（resolve_owner：id→名精确→我→唯一包含，歧义报候选）；query_single_task 补 createdAt/定时/所属工作流/依赖标题四类只读行"},
    {"id": "T1-5", "file": "src-tauri/src/bot_skills/runtime.rs", "line": 177, "fix": "Skill 回滚豁免 READONLY 清单换 query_tasks（4→3 项）"},
    {"id": "T1-6", "file": "src-tauri/src/prompts/system.rs", "line": 11, "fix": "规则 2/3/4/5 与安全红线换 query_tasks（规则 4 含 view 用法与工作流标记问答）；新增规则 22（model/owner 编辑，schedule/dependsOn 明示无入口）；prompts/mod.rs 锚点同步"},
    {"id": "T1-7", "file": "src-tauri/tests/fixtures/tools_baseline.json", "line": 1, "fix": "基线重生成为核心 28 项（tests-audit/regen_tools_baseline.py 常备脚本，抽取 SCHEMA_* 原文拼装，字节保真）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/tools.rs": 8,
    "src-tauri/src/bot/registry.rs": 2,
    "src-tauri/tests/llm_integration.rs": 2,
    "tests-audit/audit_bot_tools_alignment.py": 6
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 1
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

任务卡与数据库经 W1-CANVAS/W6-MODEL/图谱/created_at 多版本演进（30 列），agent 工具面落后：
list_tasks 只能看未完成（已完成/归档/回收站全盲）、search_tasks 与之近冗余、edit_task
够不着 model/owner 新列、子任务有 id 却只能按文本匹配。本批零新增工具（34→33），一次拉平。

## 出界（显式不做）

- schedule / dependsOn **写入口**（定时将来做在工作流侧；依赖由工作流自动生成）——只读展示保留
- archive/restore/list_people/list_workflows/query_workflow（评审中裁撤：归档恢复低频走 UI、
  工作流感知用零工具标记方案覆盖）
- origin/canvasPos/collapsed/order/assignee/budget/result/workflowId 写入口
  （纯前端布局或服务端编排专用，db/tasks.rs:791 注释明确）
