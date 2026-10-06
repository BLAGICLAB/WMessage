# Batch Spec: P2C-LIVEPAGES 工作流画布实时高亮 + 定时任务执行透明（P2-c）

```json
{
  "batch_id": "P2C-LIVEPAGES",
  "family": "exec-transparency",
  "expected_files": [
    "docs/batches/P2C-LIVEPAGES.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot_scheduler.rs",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src/components/WorkflowCanvas/TaskNode.tsx",
    "src/components/SchedulePage/SchedulePage.tsx"
  ],
  "max_lines_added": 300,
  "max_lines_removed": 40,
  "findings": [
    {"id": "P2C-1", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 538, "fix": "workflow-node-status 事件驱动 nodeLive 映射（running 高亮/failed 红环/skipped 灰环），5s workflow_is_running 轮询保留兜底；节点 onSelectTrace → TracePanel 弹层"},
    {"id": "P2C-2", "file": "src/components/WorkflowCanvas/TaskNode.tsx", "line": 28, "fix": "nodeBorder liveStatus 优先于任务行派生：running 蓝环+animate-pulse、skipped 灰环降透明（与真失败红环区分）；节点痕迹入口按钮"},
    {"id": "P2C-3", "file": "src/components/SchedulePage/SchedulePage.tsx", "line": 82, "fix": "sched-status 驱动：started 入 liveJobIds（行内「执行中…」徽标）、done/failed 出集+reload 刷 lastStatus；jobSessions 记 sessionId 供「会话」跳转（chat-focus-session + 唤起挂件）；执行历史行「痕迹」按钮按 cardId 开 TracePanel"}
  ]
}
```
