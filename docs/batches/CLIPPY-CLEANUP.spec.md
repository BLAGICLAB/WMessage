# Batch Spec: CLIPPY-CLEANUP

## 目的

clippy 全仓清理（232 行 warning → 除 vendor 外清零）：三个并行代理按互不重叠
文件组执行 + 主控补 16 站指派空隙；死代码删除均 grep 全仓零引用；WONTFIX 站
带注释标 allow。附带 Mimosa 重跑分诊记录与测试代码 OCR 扫描分诊工单（纯文档）。

## 人类可读摘要

- family: ocr-audit-closeout
- 预估 diff: 75 files modified +497/-481, new 2（本 spec + 分诊工单）

## 红线

- 最小外科修复；行为等价；死代码删除前 grep 全仓含 tests/ 零引用；
  WONTFIX 站点 allow 必须带一句中文理由；新增行无批次号 tag

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "CLIPPY-CLEANUP",
  "family": "ocr-audit-closeout",
  "expected_files": [
    "DEVLOG.md",
    "docs/MIMOSA-SCAN-TRIAGE-2026-10-07.md",
    "docs/OCR-TESTS-SCAN-TRIAGE-2026-10-08.md",
    "docs/batches/CLIPPY-CLEANUP.spec.md",
    "src-tauri/src/api_auth.rs",
    "src-tauri/src/api_handlers/body.rs",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/api_handlers/ratelimit.rs",
    "src-tauri/src/api_handlers/util.rs",
    "src-tauri/src/app_state.rs",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/io.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/schema.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/bot/mcp/config.rs",
    "src-tauri/src/bot/mcp/manager.rs",
    "src-tauri/src/bot/mcp/secrets.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot_plan.rs",
    "src-tauri/src/bot_py.rs",
    "src-tauri/src/bot_scheduler.rs",
    "src-tauri/src/bot_skills/files.rs",
    "src-tauri/src/bot_skills/manage.rs",
    "src-tauri/src/bot_skills/recommend.rs",
    "src-tauri/src/bot_skills/runtime.rs",
    "src-tauri/src/bot_skills/scheduler.rs",
    "src-tauri/src/bot_slash.rs",
    "src-tauri/src/bot_web.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/subagents.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow_audit.rs",
    "src-tauri/src/db/workspace.rs",
    "src-tauri/src/eval/runner.rs",
    "src-tauri/src/evolution/activation.rs",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/candidate/conflict.rs",
    "src-tauri/src/evolution/candidate/ttl.rs",
    "src-tauri/src/evolution/change/status.rs",
    "src-tauri/src/evolution/observe/shadow.rs",
    "src-tauri/src/evolution/observe/stop.rs",
    "src-tauri/src/evolution/panel/commands.rs",
    "src-tauri/src/evolution/sandbox/routing.rs",
    "src-tauri/src/evolution/sandbox/shadow.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/memory/consolidate.rs",
    "src-tauri/src/memory/embed.rs",
    "src-tauri/src/memory/extract.rs",
    "src-tauri/src/memory/mod.rs",
    "src-tauri/src/memory/panel.rs",
    "src-tauri/src/memory/rank.rs",
    "src-tauri/src/memory/store.rs",
    "src-tauri/src/memory/tests.rs",
    "src-tauri/src/meta/sync.rs",
    "src-tauri/src/middleware.rs",
    "src-tauri/src/migration/commands.rs",
    "src-tauri/src/migration/mod.rs",
    "src-tauri/src/migration/ops.rs",
    "src-tauri/src/migration/recovery.rs",
    "src-tauri/src/migration/rules.rs",
    "src-tauri/src/migration/run.rs",
    "src-tauri/src/ocr.rs",
    "src-tauri/src/task_autotag.rs",
    "src-tauri/src/tool_guard.rs",
    "src-tauri/src/workflow_clarify.rs",
    "src-tauri/src/workflow_decompose.rs",
    "src-tauri/src/workflow_questions.rs",
    "src-tauri/src/workflow_runner.rs",
    "src-tauri/tests/exec_trace.rs",
    "src-tauri/tests/llm_integration.rs"
  ],
  "max_lines_added": 700,
  "max_lines_removed": 650,
  "max_new_files_lines": 220,
  "findings": [],
  "assertions_min": {},
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 0
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
