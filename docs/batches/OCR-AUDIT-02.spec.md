# Batch Spec: OCR-AUDIT-02

## 目的

OpenCodeReview 全仓扫描 medium 层（bug 294 + security 34 + performance 69 = 397 条）
逐条核查后的修复批：约 63 处确认为真并修复，约 60 误报、约 30 本轮他代理先修、
其余 WONTFIX 有据。maintainability/style/doc 等建议性条目不在本批。判定全程留档
docs/OCR-SCAN-TRIAGE-2026-10-07.md 追记章节。

## 人类可读摘要

- family: ocr-scan-medium
- 覆盖 findings: 397 逐条判定（本批实修约 63 处）
- 预估 diff: 62 files / modified +631/-191, new files +0
- OCR 计划: 本批即 OCR 产出的处置，无再审轮次

## 红线

- 最小外科手术式 diff；行为变化点带可操作中文报错
- 新注释零批次号样式 tag
- 主控复核修正 2 处：ChatPanel 守卫 ref 时序（切点同步落镜像）；apply 防劫持
  测试夹具升级 512 维（M188 维度校验的正确适配，非回退校验）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "OCR-AUDIT-02",
  "family": "ocr-scan-medium",
  "expected_files": [
    ".githooks/pre-commit",
    "DEVLOG.md",
    "docs/OCR-SCAN-TRIAGE-2026-10-07.md",
    "docs/batches/OCR-AUDIT-02.spec.md",
    "scripts/ci-guard-tiny-http-vendor.sh",
    "scripts/health-check.sh",
    "scripts/install-git-hook.sh",
    "scripts/install-hooks.sh",
    "scripts/publish-docx-dotnet.sh",
    "scripts/sync-version.mjs",
    "scripts/test-all.sh",
    "scripts/triage-baseline-audit.py",
    "src-tauri/dotnet/WmDocxRevisions/Program.cs",
    "src-tauri/examples/pbtest.rs",
    "src-tauri/src/api_auth.rs",
    "src-tauri/src/api_handlers/body.rs",
    "src-tauri/src/api_handlers/ratelimit.rs",
    "src-tauri/src/api_handlers/util.rs",
    "src-tauri/src/bot/config/keyring.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/bot/mcp/commands.rs",
    "src-tauri/src/bot/mcp/secrets.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_desktop.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_skills/state.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/trace.rs",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/mod.rs",
    "src-tauri/src/evolution/sandbox/routing.rs",
    "src-tauri/src/memory/extract.rs",
    "src-tauri/src/memory/panel.rs",
    "src-tauri/src/memory/store.rs",
    "src-tauri/src/migration/commands.rs",
    "src-tauri/src/prompts/consolidate.rs",
    "src-tauri/src/py/commands.rs",
    "src-tauri/src/py/document.rs",
    "src-tauri/src/py/env.rs",
    "src-tauri/src/py/harvest.rs",
    "src-tauri/src/py/io.rs",
    "src-tauri/src/trace_sink.rs",
    "src-tauri/src/workflow_runner.rs",
    "src-tauri/tests/llm_integration.rs",
    "src-tauri/tests/memory_eval.rs",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/MessageList.tsx",
    "src/components/ChatPanel/RichText.tsx",
    "src/components/ChatPanel/UsageMeter.tsx",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/SettingsPage/MemoryPanel.tsx",
    "src/components/SettingsPage/ProfileRow.tsx",
    "src/components/SettingsPage/ProviderLogo.tsx",
    "src/components/TracePanel/TracePanel.tsx",
    "src/components/WidgetApp/SortableTaskCard.tsx",
    "src/components/WidgetApp/storage.ts",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkspacePage.tsx",
    "src/format.ts",
    "src/lib/workflowPrompt.ts",
    "src/profile.ts",
    "src/storage.ts",
    "tests-audit/regen_tools_baseline.py"
  ],
  "max_lines_added": 681,
  "max_lines_removed": 241,
  "max_new_files_lines": 199,
  "findings": [
    {
      "id": "ocr-M70",
      "file": "src-tauri/src/bot/mcp/secrets.rs",
      "line": 170,
      "fix": "read_backend_at 读故障不静默当无值"
    },
    {
      "id": "ocr-M64",
      "file": "src-tauri/src/bot/config/keyring.rs",
      "line": 486,
      "fix": "legacy 迁移先于后端写入"
    },
    {
      "id": "ocr-M127",
      "file": "src-tauri/src/db/tasks.rs",
      "line": 752,
      "fix": "task_set_column 全表扫改 PK 点查"
    },
    {
      "id": "ocr-M223",
      "file": "src-tauri/src/py/harvest.rs",
      "line": 22,
      "fix": "harvest 跳过符号链接"
    },
    {
      "id": "ocr-M315",
      "file": "src/components/ChatPanel/ChatPanel.tsx",
      "line": 698,
      "fix": "会话切换守卫 + 镜像同步落 ref"
    },
    {
      "id": "ocr-cs-372",
      "file": "src-tauri/dotnet/WmDocxRevisions/Program.cs",
      "line": 372,
      "fix": "MarkParagraphDeleted 原位标删"
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
