# Batch Spec: UI-CARD-NAV

## 目的

两个已验证的小型 UI 修复合批（worktree_clean 约束要求单批落地）：

1. 归档/回收站任务卡两处渲染 bug：备注长英文路径溢出卡片边框；「恢复」等按钮图标（Tailwind preflight `svg{display:block}`）与文字换行。
2. 侧边栏导航「定时任务」改名「定时」（老板拍板；审计确认无其他指向 schedule 页的链接入口）。

## 人类可读摘要

- family: ui-fix
- 覆盖 findings: 4（全实修）
- 预估 diff: 2 files / +9/-9 lines
- 已验证：vitest TodoCard(29)/ArchivePage(7)/App(18)/SchedulePage 全绿

## findings 原文核实

**① TodoCard.tsx:317（备注溢出，真）**：备注 `<p>` 无断行规则，`/Users/...` 长路径 token 无空格可断，撑出卡片。修法：加 `break-all`（同文件 :764 完整路径展示既有先例）。
**② TodoCard.tsx:575（归档恢复按钮换行，真）**：按钮无 `flex`，preflight 把 svg 置 block → 图标独占一行。修法：加 `flex items-center gap-1 whitespace-nowrap`，去图标 `mr-1`（同文件 :557/:622/:631 既有模式）。
**③ TodoCard.tsx:589/:597（回收站恢复/彻底删除按钮，同型隐患）**：同②修法。
**④ App.tsx:118（导航改名）**：`label: "定时任务"` → `"定时"`；:92 分区注释同步。审计：CommandPalette 无 schedule 命令、无其他 `setView("schedule")` 调用，仅此一处入口。

## 修法约束

- 纯 className/字符串改动，零逻辑、零 DOM 结构变更
- SchedulePage 内部「定时任务」字样（页标题/类型选项/aria-label）为内容概念，不改

## 自主执行规则 / Stop 条件

同既有批次约定；gate FAIL 且非 spec 调整可解即停。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "UI-CARD-NAV",
  "family": "ui-fix",
  "expected_files": [
    "src/components/TodoCard/TodoCard.tsx",
    "src/App.tsx"
  ],
  "max_lines_added": 12,
  "max_lines_removed": 12,
  "findings": [
    {"id": "UI-CARD-NAV-a", "file": "src/components/TodoCard/TodoCard.tsx", "line": 317, "fix": "备注 <p> 加 break-all，长英文路径卡内断行"},
    {"id": "UI-CARD-NAV-b", "file": "src/components/TodoCard/TodoCard.tsx", "line": 575, "fix": "归档恢复按钮加 flex items-center gap-1 whitespace-nowrap，去图标 mr-1"},
    {"id": "UI-CARD-NAV-c", "file": "src/components/TodoCard/TodoCard.tsx", "line": 589, "fix": "回收站恢复/彻底删除按钮同型修复"},
    {"id": "UI-CARD-NAV-d", "file": "src/App.tsx", "line": 118, "fix": "侧边栏导航 label 定时任务→定时，:92 注释同步"}
  ],
  "assertions_min": {
    "src/components/TodoCard/TodoCard.tsx": 0,
    "src/App.tsx": 0
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 0,
    "expected_max_comments": 0
  },
  "stop_conditions": [
    "gate_fail"
  ]
}
```

验证命令

```bash
BATCH_SPEC=docs/batches/UI-CARD-NAV.spec.md python3 scripts/batch-verify.py docs/batches/UI-CARD-NAV.spec.md
```
