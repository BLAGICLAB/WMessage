# Batch Spec: P2B-CHATENHANCE 聊天工具徽章增强 + 结构化文件摘要 + verbose 三档（P2-b）

```json
{
  "batch_id": "P2B-CHATENHANCE",
  "family": "exec-transparency",
  "expected_files": [
    "docs/batches/P2B-CHATENHANCE.spec.md",
    "DEVLOG.md",
    "src/components/ChatPanel/types.ts",
    "src/components/ChatPanel/constants.ts",
    "src/components/ChatPanel/Fold.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/InputArea.tsx",
    "src/components/ChatPanel/MessageList.tsx",
    "src/components/ChatPanel/MessageList.perf.test.tsx"
  ],
  "max_lines_added": 320,
  "max_lines_removed": 60,
  "findings": [
    {"id": "P2B-1", "file": "src/components/ChatPanel/ChatPanel.tsx", "line": 515, "fix": "bot-tool-done 消费扩展字段 result/ms/ok（P1-c 加字段兼容的兑现）；新监听 bot-file-changed 按会话累积进 streamingMeta、收尾并入最终消息（与 thinking/tools 同管线，数据流零改动）"},
    {"id": "P2B-2", "file": "src/components/ChatPanel/MessageList.tsx", "line": 178, "fix": "ToolBadges verbose 三档：concise 纯 pill / detailed 可展开入参+结果 / debug 默认展开+耗时+失败计数；FileSummary 优先结构化 fileChanges（±行），空回退正则抽取（主聊天行为不变）"},
    {"id": "P2B-3", "file": "src/components/ChatPanel/InputArea.tsx", "line": 250, "fix": "详细度 pill 点击循环 简洁→详细→调试；localStorage chat-verbose-level 持久化；仅影响展示不改消息数据流（拆分红线）"}
  ]
}
```
