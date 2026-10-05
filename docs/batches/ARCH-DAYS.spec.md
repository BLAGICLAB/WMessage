# Batch Spec: ARCH-DAYS 任务卡归档时间可配置——设置页数据管理新增天数设置（默认 7）

```json
{
  "batch_id": "ARCH-DAYS",
  "family": "settings-data-mgmt",
  "expected_files": [
    "docs/batches/ARCH-DAYS.spec.md",
    "DEVLOG.md",
    "docs/MANUAL-SMOKE-ACCEPTANCE-TASK-GRAPH-2026-10-05.md",
    "src/App.tsx",
    "src/components/ArchivePage.tsx",
    "src/components/GraphPage/graph-build.ts",
    "src/components/MigrationPanel.tsx",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/lib/archiveRule.ts",
    "src/lib/archiveRule.test.ts",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/migration/mod.rs",
    "src-tauri/src/migration/run.rs"
  ],
  "max_lines_added": 290,
  "max_lines_removed": 45,
  "max_new_files_lines": 240,
  "findings": [
    {"id": "ARCH-1", "file": "src-tauri/src/bot/config/types.rs", "line": 148, "fix": "BotConfig 新增 Option<u32> archive_after_days（serde default + skip_serializing_if，老配置缺字段兼容、None 不落盘）；BotConfigView 透传；DEFAULT_ARCHIVE_DAYS=7 / MAX_ARCHIVE_DAYS=365 常量 + resolve_archive_after_days（None→7，有值钳 1..=365）"},
    {"id": "ARCH-2", "file": "src-tauri/src/bot/config/commands.rs", "line": 30, "fix": "archive_after_days(app) 读取侧封装（migration 与前端同源）；bot_get_config 视图携带 archiveAfterDays；bot_set_config 落盘前钳 Some(n)→clamp(1,365)"},
    {"id": "ARCH-3", "file": "src-tauri/src/migration/run.rs", "line": 75, "fix": "删硬编码 ARCHIVE_AFTER_MS 常量；阶段一归档阈值改为每轮 crate::bot::archive_after_days(app) 现读（单轮内一致；主窗口关闭时兜底归档与前端同阈值）"},
    {"id": "ARCH-4", "file": "src/lib/archiveRule.ts", "line": 1, "fix": "新建：applyArchiveRule + 天数内存缓存（get/set/clampArchiveDays，钳制与后端同规则）+ loadArchiveDaysFromConfig（invoke bot_get_config，失败保持现值）。存 bot-config 而非 localStorage：后端轮询线程必须读到同一份阈值"},
    {"id": "ARCH-5", "file": "src/App.tsx", "line": 231, "fix": "启动在首套规则前 loadArchiveDaysFromConfig()（防首屏按默认 7 打错标）；bot-config-changed 监听里 setArchiveAfterDays + 立即重套规则（阈值调小即刻归档，不等 60s 定时器）"},
    {"id": "ARCH-6", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 411, "fix": "数据管理卡新增「任务卡归档天数」输入（1–365，默认 7，aria-label 恒定）+ 本卡保存钮；保存载荷空/非法=null（后端回默认 7）、越界钳 365"},
    {"id": "ARCH-7", "file": "src/components/SettingsPage.test.tsx", "line": 173, "fix": "新测：默认 7 显示、改 30 保存载荷携带、500 钳 365"},
    {"id": "ARCH-8", "file": "src/lib/archiveRule.test.ts", "line": 1, "fix": "新 9 测：钳制（0/负/超界/NaN/Infinity）、缓存 set/get、applyArchiveRule 按配置阈值归档（只动 done 列、老数据补 completedAt）"},
    {"id": "ARCH-9", "file": "src-tauri/src/bot/config/mod.rs", "line": 404, "fix": "pub use 透传 archive_after_days/resolve_archive_after_days/DEFAULT_ARCHIVE_DAYS/MAX_ARCHIVE_DAYS；config 模块新 2 测（resolve 钳制 + 老配置缺字段兼容往返、None 不落盘）"},
    {"id": "ARCH-10", "file": "src/components/ArchivePage.tsx", "line": 97, "fix": "归档页空态文案去硬编码：显示当前配置天数"},
    {"id": "ARCH-11", "file": "src/components/MigrationPanel.tsx", "line": 106, "fix": "桌面清理说明文案去硬编码：显示当前配置天数"},
    {"id": "ARCH-12", "file": "src-tauri/src/bot.rs", "line": 61, "fix": "pub use 透传 archive_after_days/resolve_archive_after_days/DEFAULT_ARCHIVE_DAYS/MAX_ARCHIVE_DAYS"},
    {"id": "ARCH-13", "file": "docs/MANUAL-SMOKE-ACCEPTANCE-TASK-GRAPH-2026-10-05.md", "line": 58, "fix": "图谱冒烟清单：勾选部门视图/成员过滤/Obsidian 交互三项（r5 真机验收结果）；新增 4b 节——G4/G5/G6 聚簇·视野自适应·依赖编辑·标签近义人工验收项（含近义降级）"}
  ],
  "assertions_min": {
    "src/lib/archiveRule.test.ts": 0
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 1
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

归档阈值原为硬编码 7 天（前端 App.tsx ARCHIVE_AFTER_MS + 后端
migration::ARCHIVE_AFTER_MS 两处各写一份）。设置页「数据管理」新增
「任务卡归档时间」，默认 7 天，可设 1–365 天。配置存 bot-config.json
（archiveAfterDays）——后端 migration 兜底归档跑在 Rust 轮询线程，必须
与前端 applyArchiveRule 同源读同一份阈值，否则出现「界面 30 天、后端
7 天照归」的分叉。前端新建 lib/archiveRule.ts（规则 + 天数缓存 + 配置
加载），App 启动在首套规则前加载、bot-config-changed 广播时即时重套。

附带收尾入库：DEVLOG ARCH-DAYS 条目；图谱冒烟清单 r5 验收勾选 + 4b
增批小节（graph-build.ts 注释同步「归档天数可改」口径——worktree_clean
门禁要求本批一并带上）。
