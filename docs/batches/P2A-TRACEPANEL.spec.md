# Batch Spec: P2A-TRACEPANEL 执行详情面板 + 文件级回滚（Agent 透明化 P2-a）

```json
{
  "batch_id": "P2A-TRACEPANEL",
  "family": "exec-transparency",
  "expected_files": [
    "docs/batches/P2A-TRACEPANEL.spec.md",
    "DEVLOG.md",
    "src-tauri/src/db/trace.rs",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/lib.rs",
    "src/lib/trace.ts",
    "src/components/TracePanel/TracePanel.tsx",
    "src/components/TracePanel/DiffView.tsx",
    "src/components/TracePanel/index.ts",
    "src/components/TracePanel/TracePanel.test.tsx",
    "src/components/TodoCard/TodoCard.tsx"
  ],
  "max_lines_added": 560,
  "max_lines_removed": 40,
  "findings": [
    {"id": "P2A-1", "file": "src-tauri/src/db/trace.rs", "line": 452, "fix": "file_rollback 双闸：漂移闸（当前文件 sha == after_sha，防吞 AI 修改后的人工改动）+ 快照闸（before_sha）；仅 modify；atomic_write 回写 + file.rollback 审计"},
    {"id": "P2A-2", "file": "src/components/TracePanel/TracePanel.tsx", "line": 1, "fix": "createPortal 到 body（TodoCard hover transform 创建 CSS 包含块会裁缩 fixed 子元素——purge 弹窗同款教训）；摘要头/执行历史 pills/文件修改区（±行+diff+回滚）/工具时间线"},
    {"id": "P2A-3", "file": "src/components/TracePanel/DiffView.tsx", "line": 1, "fix": "unified diff 行着色 ~30 行解析器，不引第三方 diff 库（后端 similar 产出 unified 文本）"}
  ]
}
```
