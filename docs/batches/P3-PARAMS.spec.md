# Batch Spec: P3-PARAMS 参数透明——注册表 + 配置化接线 + 设置页参数卡 + 词元统计（P3-a/b）

```json
{
  "batch_id": "P3-PARAMS",
  "family": "agent-transparency",
  "expected_files": [
    "docs/batches/P3-PARAMS.spec.md",
    "DEVLOG.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/params.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/lib.rs",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/UsageStatsCard.tsx",
    "src/components/SettingsPage/UsageStatsCard.test.tsx"
  ],
  "max_lines_added": 780,
  "max_lines_removed": 60,
  "findings": [
    {"id": "P3A-1", "file": "src-tauri/src/bot/params.rs", "line": 1, "fix": "PARAMS_TABLE 单源 21 项（可编辑 10 + 硬编码只读 11）；resolve_* 读取口唯一（钳制内聚）；bot_effective_params 出生效值/默认/来源三态"},
    {"id": "P3A-2", "file": "src-tauri/src/bot/config/types.rs", "line": 209, "fix": "BotConfig/View 增 7 个全 Option 字段（serde default 老配置零影响无需 bump）；bot_set_config 落盘前钳制（archive 同款）"},
    {"id": "P3A-3", "file": "src-tauri/src/bot_model_loop.rs", "line": 303, "fix": "resolve_max_rounds 增 config 参数（Skill 自报 > 配置 > 默认三层）；DEFAULT_MAX_ROUNDS 常量单源改指 params"},
    {"id": "P3A-4", "file": "src-tauri/src/bot_orchestrator.rs", "line": 328, "fix": "spawn_subagent 预算 None → 配置默认（resolve 内钳制）；Gate 并发 max_running 保持常量（轮询路径不做 IO，留档）"},
    {"id": "P3B-1", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1940, "fix": "三链透传 7 参数（防整份替换写抹字段——与 P3-a 同批的原因）+「Agent 运行参数」卡（7 行输入 hint 带钳制区间与代价）"},
    {"id": "P3B-2", "file": "src/components/SettingsPage/UsageStatsCard.tsx", "line": 1, "fix": "词元统计实装：usage_stats_daily 按日聚合，汇总四格 + 双色条形（纯 div），纯本地无上报"}
  ]
}
```
