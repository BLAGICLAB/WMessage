# Batch Spec: P4A-TRACETOOLS 截断落地 + error_class 分类器 + 痕迹清理/导出（P4 首批）

```json
{
  "batch_id": "P4A-TRACETOOLS",
  "family": "agent-transparency",
  "expected_files": [
    "docs/batches/P4A-TRACETOOLS.spec.md",
    "DEVLOG.md",
    "src-tauri/src/audit.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/db/trace.rs",
    "src-tauri/src/lib.rs",
    "src/lib/trace.ts",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/TracePanel/TracePanel.tsx"
  ],
  "max_lines_added": 480,
  "max_lines_removed": 40,
  "findings": [
    {"id": "P4A-1", "file": "src-tauri/src/bot_model_loop.rs", "line": 625, "fix": "maxToolOutputChars 截断落地：薄壳闭包读 config（零额外 IO），Some(n>0) 钳 n 字符（上限 200K）+ tool.output.truncated 审计；默认 None=不截断零变更；span 存原文、其余字段透传"},
    {"id": "P4A-2", "file": "src-tauri/src/audit.rs", "line": 143, "fix": "classify_error_class 与 tool_call_failed 同源口径（成功 None 不私自扩面）；两处消费：ToolCallSummary.error_kind + dispatch span error_class"},
    {"id": "P4A-3", "file": "src-tauri/src/db/trace.rs", "line": 680, "fix": "trace_export：单次执行导出 data_dir/exports/trace-<id>.jsonl（type 字段区分 trace/span/file_change 行）；atomic_write；trace.export 审计"}
  ]
}
```
