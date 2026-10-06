# Batch Spec: OCR-AUDIT-01

## 目的

OpenCodeReview 全仓扫描（328 文件 / 1501 评论）critical/high 共 337 条逐条核查后的
统一修复批：约 130 处确认为真并修复（Rust / TS / C# / 脚本 / 治具），约 55 误报、
15 已有防线、110 WONTFIX（有据）、13 NEEDS-HUMAN——全部判定与证据留档
docs/OCR-SCAN-TRIAGE-2026-10-07.md，原始扫描报告存档 docs/ocr-scan-report-2026-10-07.md。

## 人类可读摘要

- family: ocr-scan-critical-high（跨域审计修复批，family 即「OCR critical/high 核查产出」）
- 覆盖 findings: 337 逐条判定（本批实修约 130 处）
- 预估 diff: 127 files / modified +1931/-574, new files +26148
- OCR 计划: 本批即 OCR 产出的处置，无再审轮次（expected_max_comments=0）

## 红线

- 最小外科手术式 diff；行为变化点必须带可操作中文报错
- 新注释零批次号样式 tag（pre-commit 红线扫描）
- 主控复核修正：migrations `Ok(_)`、bot_fs 闭包签名、harvest 未来 mtime 语义回退
  （与既有测试钉死语义一致）、bot.rs 死 import 清理

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "OCR-AUDIT-01",
  "family": "ocr-scan-critical-high",
  "expected_files": [
    ".cargo/config.toml",
    ".githooks/pre-commit",
    ".zcodeignore",
    "DEVLOG.md",
    "docs/OCR-SCAN-TRIAGE-2026-10-07.md",
    "docs/batches/OCR-AUDIT-01.spec.md",
    "docs/ocr-scan-report-2026-10-07.md",
    "gui-test-screenshots/graph-demo-entry.tsx",
    "gui-test-screenshots/graph-demo.html",
    "scripts/batch-verify.py",
    "scripts/fetch_ocr_models.sh",
    "scripts/install-git-hook.sh",
    "scripts/test-all.sh",
    "src-tauri/dotnet/WmDocxRevisions/Program.cs",
    "src-tauri/src/api.rs",
    "src-tauri/src/api_auth.rs",
    "src-tauri/src/api_handlers/mod.rs",
    "src-tauri/src/api_handlers/sse.rs",
    "src-tauri/src/app_state.rs",
    "src-tauri/src/audit.rs",
    "src-tauri/src/bin/observe_run.rs",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/keyring.rs",
    "src-tauri/src/bot/config/schema.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/bot/mcp/commands.rs",
    "src-tauri/src/bot/params.rs",
    "src-tauri/src/bot/reasoning.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/bot_anthropic.rs",
    "src-tauri/src/bot_artifacts.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_desktop.rs",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot_scheduler.rs",
    "src-tauri/src/bot_skills/files.rs",
    "src-tauri/src/bot_skills/manage.rs",
    "src-tauri/src/bot_skills/parse.rs",
    "src-tauri/src/bot_skills/runtime.rs",
    "src-tauri/src/bot_skills/state.rs",
    "src-tauri/src/bot_web.rs",
    "src-tauri/src/db/bot_history.rs",
    "src-tauri/src/db/bot_sessions.rs",
    "src-tauri/src/db/migrations.rs",
    "src-tauri/src/db/paths.rs",
    "src-tauri/src/db/schedule_jobs.rs",
    "src-tauri/src/db/trace.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/eval/config.rs",
    "src-tauri/src/eval/metrics.rs",
    "src-tauri/src/eval/sampler.rs",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/derive.rs",
    "src-tauri/src/evolution/emit.rs",
    "src-tauri/src/evolution/observe/shadow.rs",
    "src-tauri/src/evolution/observe/stop.rs",
    "src-tauri/src/evolution/observe/synthetic.rs",
    "src-tauri/src/evolution/panel/commands.rs",
    "src-tauri/src/evolution/policy.rs",
    "src-tauri/src/evolution/sandbox/io.rs",
    "src-tauri/src/evolution/trace.rs",
    "src-tauri/src/exec_steps.rs",
    "src-tauri/src/memory/consolidate.rs",
    "src-tauri/src/memory/extract.rs",
    "src-tauri/src/memory/panel.rs",
    "src-tauri/src/memory/rank.rs",
    "src-tauri/src/memory/store.rs",
    "src-tauri/src/memory/tests.rs",
    "src-tauri/src/middleware.rs",
    "src-tauri/src/migration/commands.rs",
    "src-tauri/src/migration/ops.rs",
    "src-tauri/src/migration/recovery.rs",
    "src-tauri/src/migration/rules.rs",
    "src-tauri/src/notifications.rs",
    "src-tauri/src/ocr.rs",
    "src-tauri/src/paths.rs",
    "src-tauri/src/platform/copy_file.rs",
    "src-tauri/src/profile.rs",
    "src-tauri/src/prompts/consolidate.rs",
    "src-tauri/src/py/audit.rs",
    "src-tauri/src/py/env.rs",
    "src-tauri/src/py/harvest.rs",
    "src-tauri/src/py/runtime.rs",
    "src-tauri/src/tag_similar.rs",
    "src-tauri/src/task_autotag.rs",
    "src-tauri/src/tool_guard.rs",
    "src-tauri/src/workflow_decompose.rs",
    "src-tauri/src/workflow_runner.rs",
    "src/App.tsx",
    "src/components/ActivityPage/index.tsx",
    "src/components/ArchivePage.tsx",
    "src/components/ChatPanel/Fold.tsx",
    "src/components/ChatPanel/InputArea.tsx",
    "src/components/ChatPanel/SessionList.tsx",
    "src/components/ChatPanel/useChatUi.ts",
    "src/components/ConfirmMap/ConfirmMap.tsx",
    "src/components/DoneCircle.tsx",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/graph-adapter.ts",
    "src/components/MarkdownText.tsx",
    "src/components/NotificationsPage/NotificationsPage.tsx",
    "src/components/SchedulePage/SchedulePage.tsx",
    "src/components/SettingsPage/MemoryPanel.tsx",
    "src/components/SettingsPage/ProviderLogo.tsx",
    "src/components/SettingsPage/SkillsPanel.tsx",
    "src/components/SettingsPage/UsageStatsCard.tsx",
    "src/components/TodoCard/SubtaskRow.tsx",
    "src/components/TodoCard/TodoCard.tsx",
    "src/components/TracePanel/TracePanel.tsx",
    "src/components/TrashPage.tsx",
    "src/components/WidgetApp/SplitBar.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/components/WorkflowCanvas/GoalNode.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/lib/graphPrefs.ts",
    "src/profile.ts",
    "src/test/setup.ts",
    "src/ui/ErrorBoundary.tsx",
    "src/ui/ErrorDialogHost.tsx",
    "src/ui/main.css",
    "tests-audit/audit_bot_tools_alignment.py",
    "tests-audit/audit_error_codes.py",
    "tests-audit/audit_pre_step_pre_execute.py",
    "tests-audit/regen_tools_baseline.py"
  ],
  "max_lines_added": 1981,
  "max_lines_removed": 624,
  "max_new_files_lines": 26412,
  "findings": [
    {
      "id": "ocr-F74",
      "file": "src-tauri/src/bot_fs.rs",
      "line": 509,
      "fix": "resolve_writable 目标末段软链 fail-closed"
    },
    {
      "id": "ocr-F219",
      "file": "src-tauri/src/workflow_decompose.rs",
      "line": 383,
      "fix": "附件 invoke 边界校验 canonicalize+扩展集"
    },
    {
      "id": "ocr-F30",
      "file": "src-tauri/src/api_handlers/sse.rs",
      "line": 148,
      "fix": "SSE writer 持锁内联注册消空窗"
    },
    {
      "id": "ocr-F51",
      "file": "src-tauri/src/bot/config/keyring.rs",
      "line": 169,
      "fix": "降级 key 文件原子写"
    },
    {
      "id": "ocr-F81",
      "file": "src-tauri/src/bot_model_loop.rs",
      "line": 261,
      "fix": "tool_call arguments 1MiB 上限"
    },
    {
      "id": "ocr-F0cs",
      "file": "src-tauri/dotnet/WmDocxRevisions/Program.cs",
      "line": 587,
      "fix": "ReadDocxLines 口径与在地球对齐（OCR 原文方向反转，实修镜像）"
    }
  ],
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
