# Batch Spec: U20A-BTNUNIFY

## 目的

功能按键风格统一（U20 波收尾，老板 2026-10-04 拍板「自进化刷新键太丑」）：
全仓普查确认两大家族——`nm-btn`（浮雕文字键）与 `IconButton`/`.nm-icon-btn`
（扁平图标键，lucide 13px + `--t5` hover `--t2`）——EvolutionPanel 与
SettingsPage 各有家族外孤例，本批归一：

- 自进化头部刷新键：`nm-btn` + 「🔄 刷新」文字 → `IconButton` + `RefreshCw
  size=13`（对齐 MemoryPanel 头部刷新键形态，aria-label + title 齐备）；
- EvolutionPanel 去 emoji 前缀（全仓孤例）：🪞 立即反思 / ✅ 启用 / ⛔ 停用 /
  ⏳ 延长 shadow / ↩️ 回滚 → 纯文字 `nm-btn`（与 MigrationPanel/SkillsPanel/
  MemoryPanel 等全仓 `nm-btn` 无 emoji 口径一致）；
- SettingsPage「⟳ 更新模型库」去符号（两处 + 同步注释）→ 纯文字。

## 普查结论（不做部分）

- `nm-btn` 家族：47 处 className 形态聚类后完全一致（px/py 尺寸梯度 +
  `--t2/t3/t4/t5` 字色 + `--danger` 危险色），除上述 emoji/符号孤例外零漂移；
- `IconButton` 家族：MemoryPanel（3 头部 + 2 行内）、McpPanel（2）、
  WorkspacePage（3）形态一致（aria-label + title + 13px + `--t5`/hover `--t2`），
  本批 EvolutionPanel 加入同款；
- ChatPanel 的 emoji（⏳/⚠️）出现在提示消息文案，非按钮，不属本批范围；
- FilterBtn（nm-inset/nm-outset 筛选钮）、Toggle、DeleteConfirmDialog 各自
  家族内部一致，不动。

## 验证

- vitest 401/401 全绿（EvolutionPanel 测试对 emoji 零引用，断言全走
  正则/testid，存量测试零改动）；test-fast.sh exit 0（tsc/knip/oxlint/桥审计）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20A-BTNUNIFY",
  "family": "ui-unify",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20A-BTNUNIFY.spec.md",
    "src/components/EvolutionPanel/EvolutionPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 120,
  "max_lines_removed": 40,
  "max_new_files_lines": 120,
  "findings": [
    { "file": "src/components/EvolutionPanel/EvolutionPanel.tsx", "note": "头部刷新键换 IconButton（RefreshCw 13px，--t5 hover --t2，aria+title 齐备）；五处 emoji 前缀去除归一 nm-btn 纯文字口径" },
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "「⟳ 更新模型库」去符号（两处 + 注释同步），对齐全仓 nm-btn 无符号口径" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx vitest --run src/components/EvolutionPanel src/components/SettingsPage   # 相关全绿
bash scripts/test-fast.sh                                                    # exit 0
python3 scripts/batch-verify.py docs/batches/U20A-BTNUNIFY.spec.md           # 过
```
