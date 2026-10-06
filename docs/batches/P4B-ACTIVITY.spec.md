# Batch Spec: P4B-ACTIVITY Profiles 三预设 + 水位条 + 主窗活动页 + evolution 采样接线（P4 收尾批）

```json
{
  "batch_id": "P4B-ACTIVITY",
  "family": "agent-transparency",
  "expected_files": [
    "docs/batches/P4B-ACTIVITY.spec.md",
    "DEVLOG.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src/components/ChatPanel/types.ts",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/UsageMeter.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/TracePanel/TracePanel.tsx",
    "src/components/ActivityPage/index.tsx",
    "src/App.tsx"
  ],
  "max_lines_added": 620,
  "max_lines_removed": 50,
  "findings": [
    {"id": "P4B-1", "file": "src-tauri/src/bot_chat.rs", "line": 1468, "fix": "run_task_in_chat 闭包喂 maybe_record_trace（Success 带 tool_calls/Failure 带 reason；session 标识用 task_id）；TaskExecOrigin→MutationOrigin::Bot"},
    {"id": "P4B-2", "file": "src-tauri/src/bot_model_loop.rs", "line": 578, "fix": "llm.usage 薄壳拦截处 emit bot-usage-delta（仅交互实例推挂件；OpenAI stream_options 不启用留档——水位仅 Anthropic 协议有数据）"},
    {"id": "P4B-3", "file": "src/components/ChatPanel/UsageMeter.tsx", "line": 1, "fix": "水位条：Σ tokens + contextK 百分比 + ≥80% 红色提示 /compact；无数据整条隐藏不接假数据"},
    {"id": "P4B-4", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1945, "fix": "Profiles 三预设按钮（保守/标准/放开）一键整组 saveConfig；纯前端联动写后端零改动"},
    {"id": "P4B-5", "file": "src/components/ActivityPage/index.tsx", "line": 1, "fix": "活动聚合页：trace_list 最近 50 + running 脉冲 10s 轮询 + 行点击 TracePanel traceId 直查模式 + 唤起挂件按钮"}
  ]
}
```
