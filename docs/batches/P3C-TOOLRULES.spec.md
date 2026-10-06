# Batch Spec: P3C-TOOLRULES per-tool 权限规则表 + 授权模式 auto 档（P3-c）

```json
{
  "batch_id": "P3C-TOOLRULES",
  "family": "agent-transparency",
  "expected_files": [
    "docs/batches/P3C-TOOLRULES.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/bot/params.rs",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/ChatPanel/types.ts",
    "src/components/ChatPanel/constants.ts",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/InputArea.tsx"
  ],
  "max_lines_added": 560,
  "max_lines_removed": 50,
  "findings": [
    {"id": "P3C-1", "file": "src-tauri/src/bot_fs.rs", "line": 33, "fix": "lookup_tool_rule 纯函数：deny>ask>allow 首中即停，脏 action 防御性视为无命中；allow 在两个 resolve 的白名单判定之后接线（单工具 yolo 语义，不绕过白名单判定——无新增逃逸面）"},
    {"id": "P3C-2", "file": "src-tauri/src/bot/dispatch.rs", "line": 179, "fix": "规则评估入口：deny 硬拒（early_return 配平 + tool_rule.hit 审计）；ask 强制确认；use_skill 豁免"},
    {"id": "P3C-3", "file": "src-tauri/src/bot_fs.rs", "line": 890, "fix": "write_file 覆盖确认 auto 档跳过（白名单内 acceptEdits 核心语义）；yolo 维持既有弹窗（默认行为零变更）"},
    {"id": "P3C-4", "file": "src-tauri/src/bot/config/commands.rs", "line": 140, "fix": "sanitize_tool_rules：空名/非法 action/重复 tool（按 trim 后名）剔除、空表归一 None"},
    {"id": "P3C-5", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 2620, "fix": "授权卡四档按钮（新增「白名单内自动」）+ 规则表行编辑（工具名+动作下拉+删除）；三链透传 toolRules（空表传 null）"}
  ]
}
```
