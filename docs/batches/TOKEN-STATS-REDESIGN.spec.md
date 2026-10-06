# Batch Spec: TOKEN-STATS-REDESIGN 词元统计卡按「使用统计」参照图重排 + 按模型采数链路

```json
{
  "batch_id": "TOKEN-STATS-REDESIGN",
  "family": "feat",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/TOKEN-STATS-REDESIGN.spec.md",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/trace.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/trace_sink.rs",
    "src/App.tsx",
    "src/bubbleStyle.ts",
    "src/bubbleStyle.test.ts",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/MessageList.tsx",
    "src/components/ChatPanel/UsageMeter.tsx",
    "src/components/ChatPanel/UserBubbleContent.tsx",
    "src/components/SettingsPage/BubbleStyleCard.tsx",
    "src/components/SettingsPage/BubbleStyleCard.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/UsageStatsCard.tsx",
    "src/components/SettingsPage/UsageStatsCard.test.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/ui/main.css"
  ],
  "max_lines_added": 2100,
  "max_lines_removed": 250,
  "max_new_files_lines": 700,
  "findings": [
    {"id": "TSR-1", "file": "src/components/SettingsPage/UsageStatsCard.tsx", "line": 1, "fix": "词元统计卡重排：汇总 5 项（含连续天数游程计算）→ 年宽 7×N 周热力图（每日/每周/累计三态 + 月份标签）→ 时间范围行 + 按模型分线趋势（前 5 + 未知模型）→ 模型用量独立卡；零图表库"},
    {"id": "TSR-2", "file": "src-tauri/src/db/trace.rs", "line": 656, "fix": "新增 usage_stats_daily_by_model(_conn) 命令：day×model 聚合、day 升序、model NULL 保留，days 钳 1..=365"},
    {"id": "TSR-3", "file": "src-tauri/src/bot_chat.rs", "line": 1, "fix": "主聊天也落 exec_traces（origin=chat）：begin/end 成对、LoopTrace 统计塞 TraceCapture，纯聊天用户词元统计不再恒空"},
    {"id": "TSR-4", "file": "src-tauri/src/db/mod.rs", "line": 184, "fix": "open_db 改接 ensure_trace_tables（原只跑裸 DDL）——旧库缺 model 列时 trace_finish 报 no such column，exec_trace 集成测试两条生命周期用例红"},
    {"id": "TSR-5", "file": "src/components/SettingsPage/BubbleStyleCard.tsx", "line": 1, "fix": "同批遗留前端工作一并入库：聊天气泡样式自定义（用户气泡颜色 + 机器人气泡材质，纯前端偏好即点即生效）+ 水位条口径改按最近一轮 input"}
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

设置页「词元统计」卡按「使用统计」参照图重排：汇总指标 5 项竖线分隔（新增
当前/最长连续天数）、年宽 GitHub 式热力图（每日/每周/累计三态 + 月份标签）、
时间范围行卡外化、每日趋势改按模型分线、模型用量独立卡。趋势分线依赖新增
`usage_stats_daily_by_model` 命令与主聊天 trace 落库（此前纯聊天用户统计恒空）。
顺手修复 open_db 漏接 `ensure_trace_tables` 导致旧库 model 列缺失、
exec_trace 集成测试红的 WIP 遗留。工作区同时遗留聊天气泡样式自定义前端工作
（BubbleStyleCard/bubbleStyle + ChatPanel 接线），与卡片改动同树，一并入库。
