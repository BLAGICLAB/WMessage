# Batch Spec: U13A-MAXTOKEN-UI

## 目的

厂商设置页底部的全局 max_tokens 输入删除（老板 2026-10-03 截图指派）：每个
模型编辑打开时都有 max_tokens 设置（U11 起），页底再显示一次重复且歧义
（看似厂商级，实为协议级兜底）。删除范围仅 UI——`BotConfig.maxTokens`
字段、loadConfig 读取、saveConfig 透传全部保留：已存值继续作「条目留空时」
的兜底层（条目 > 全局 > 8192 默认），无数据丢失、无迁移。插头说明文案里
「max_tokens 在 Anthropic 格式下按厂商设置」同步改为指向各模型编辑
（仅 Anthropic 格式生效）。

## 处置记录

- 兜底层无 UI 入口后的语义：条目 maxTokens 留空 → 回退盘上历史全局值
  （如有）→ 8192 默认；新用户不可再配置全局层（per-model 即唯一入口，
  与 U13 的条目级接线一致）。
- 透传测试改断言 `maxTokens: null`（未配置原样透传，防未来误删透传通道）。

## 红线核对

- 后端零改动；keyring 零改动；verified_vendors / prune 零改动。
- ModelRow 编辑态的每模型 max_tokens 输入与「仅 Anthropic 格式生效」
  说明行不动。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U13A-MAXTOKEN-UI",
  "family": "ui-cleanup",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U13A-MAXTOKEN-UI.spec.md",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 40,
  "max_new_files_lines": 60,
  "findings": [
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "全局 max_tokens 输入删除；state/透传保留作兜底层（条目 > 全局 > 8192），无 UI 入口" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx --no-install vitest --run SettingsPage   # 63 绿
bash scripts/test-fast.sh                    # exit 0
```
