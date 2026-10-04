# Batch Spec: U20C-ICON1A

## 目的

U20C-ICON1 的换行回归修复（老板 2026-10-04 截图反馈：定时/导入/导出/选择图片/
查看迁移日志/导入技能文件夹/打开技能目录等按钮图标与文字折行）。根因：emoji 是
单文本节点内的字符，lucide SVG + 空格 + 文本产生了**可断行点**——无 flex 的按钮
（定时、导出导入等）在图标处折行；有 flex 的按钮内容在挤压时也会内部折行。

## 修法（统一口径）

凡「图标 + 文字」按钮补两类类名：
- 缺 flex 的（定时×2、导出×2、导入×2、主题三档、保存×2、复制、导入技能文件夹、
  打开技能目录、选择图片、下载表格模版、导入规则表、查看迁移日志、重试）→
  `inline-flex items-center gap-1 whitespace-nowrap`（居中类按钮加
  `justify-center`，保持 min-w-[76px] 按钮的文字居中观感）；
- 已有 flex 的（交给机器人×2、绑定文件/文件夹）→ 补 `whitespace-nowrap`；
- 已归档（KanbanBoard 文本键）→ 补 `whitespace-nowrap`。

whitespace-nowrap 后按钮 min-content = 整行内容宽度，flex 挤压时按钮不再收缩
折行（行为回到 emoji 时代的单行形态）；行内 chip（truncate 容器内）不受影响。

## 验证

vitest 401/401 全绿；tsc + test-fast.sh exit 0。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20C-ICON1A",
  "family": "ui-icon-migration",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20C-ICON1A.spec.md",
    "src/components/KanbanBoard.tsx",
    "src/components/MigrationPanel.tsx",
    "src/components/SettingsPage/ProfileRow.tsx",
    "src/components/SettingsPage/SkillsPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/TaskCardContent.tsx",
    "src/components/TodoCard/TodoCard.tsx",
    "src/ui/ErrorBoundary.tsx"
  ],
  "max_lines_added": 80,
  "max_lines_removed": 40,
  "max_new_files_lines": 80,
  "findings": [
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "导出导入×4/主题三档/保存×2/复制 全部补 inline-flex + whitespace-nowrap（居中键带 justify-center 保 min-w 观感）" },
    { "file": "src/components/TaskCardContent.tsx", "note": "定时按钮（截图中折行处）补 inline-flex items-center gap-1 whitespace-nowrap；交给机器人补 nowrap" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx vitest --run                                  # 401/401
bash scripts/test-fast.sh                         # exit 0
python3 scripts/batch-verify.py docs/batches/U20C-ICON1A.spec.md
```
