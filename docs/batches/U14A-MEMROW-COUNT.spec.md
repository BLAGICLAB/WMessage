# Batch Spec: U14A-MEMROW-COUNT

## 目的

老板看板反馈（U14 上线后截图）：记忆行的「被想起 N 次」原来只收在悬停
title 里，等于看不见。改为行内直显「想起 N」（accessCount>0 才渲染，
0 次不占位）；title 同步改为口径说明「被聊天注入命中的次数」（面板搜索
不计入——U14 红线）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U14A-MEMROW-COUNT",
  "family": "ui-cleanup",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U14A-MEMROW-COUNT.spec.md",
    "src/components/SettingsPage/MemoryPanel.test.tsx",
    "src/components/SettingsPage/MemoryPanel.tsx"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 15,
  "max_new_files_lines": 50,
  "findings": [
    { "file": "src/components/SettingsPage/MemoryPanel.tsx", "note": "accessCount 行内直显（>0 才渲染）；title 改口径说明（面板搜索不计入）" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx --no-install vitest --run MemoryPanel   # 7 绿
bash scripts/test-fast.sh                   # exit 0
```
