# Batch Spec: N1-NOTIFCENTER Agent 通知中心——三类决策事件持久化消息化

```json
{
  "batch_id": "N1-NOTIFCENTER",
  "family": "agent-notifications",
  "expected_files": [
    "docs/batches/N1-NOTIFCENTER.spec.md",
    "DEVLOG.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/notifications.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/memory/extract.rs",
    "src-tauri/src/evolution/mod.rs",
    "src-tauri/src/evolution/candidate/mod.rs",
    "src-tauri/src/evolution/panel/commands.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_artifacts.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot/tools.rs",
    "src-tauri/src/prompts/system.rs",
    "src-tauri/src/prompts/execute.rs",
    "src-tauri/tests/fixtures/tools_baseline.json",
    "src-tauri/tests/evolution_gov.rs",
    "src/components/NotificationsPage/NotificationsPage.tsx",
    "src/components/NotificationsPage/NotificationsPage.test.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/App.tsx"
  ],
  "max_lines_added": 1200,
  "max_lines_removed": 400,
  "findings": [
    {"id": "N1-1", "file": "src-tauri/src/notifications.rs", "line": 48, "fix": "notifications 表（TEXT PK 幂等 id / INSERT OR IGNORE / resolve 仅动 pending 行）+ 四命令（全 ?N 参数绑定）+ notifications-changed 广播 + 5 单测"},
    {"id": "N1-2", "file": "src-tauri/src/memory/extract.rs", "line": 699, "fix": "confirm 档入队落「新记忆提案 · N 条」（memory:{session}:{ts} 幂等）+ approve/reject 尾部 notif_sync_memory（局部收下 payload 收缩）"},
    {"id": "N1-3", "file": "src-tauri/src/evolution/mod.rs", "line": 137, "fix": "提案入池逐条落消息（evo:{proposal_id}）；write_proposals 返回新写入条目供逐条落消息；auto 档即将自动应用的不打扰（:187-192 分流）"},
    {"id": "N1-4", "file": "src-tauri/src/bot_chat.rs", "line": 1569, "fix": "执行收尾落「任务「X」完成，N 个产物待绑定」（artifact:{task_id}:{sid}，payload 随 paths 落库重启可补绑）；删除挂件 artifact-batch-ready 弹窗事件"},
    {"id": "N1-5", "file": "src/components/NotificationsPage/NotificationsPage.tsx", "line": 1, "fix": "通知页：待处理/全部两页签，三类消息卡操作（记忆收下/忽略、进化启用/忽略、产物卡内嵌勾选默认全选）+ 导航 Bell 角标（99+ 封顶）"},
    {"id": "N1-6", "file": "src-tauri/src/bot/registry.rs", "line": 173, "fix": "验收修复：SCHEMA_LINK_FILE_TO_TASK 等 5 处模型可见文案「弹汇总窗口」→「通知中心」措辞（弹窗已删，baseline 前缀重生成）"},
    {"id": "N1-7", "file": "src-tauri/tests/evolution_gov.rs", "line": 209, "fix": "验收修复：断言跟进 write_proposals 新签名（Vec.len + 返回条目 proposal_id）——此前 cargo test 全量在编译期中止"}
  ],
  "assertions_min": {
    "src-tauri/src/notifications.rs": 5,
    "src/components/NotificationsPage/NotificationsPage.test.tsx": 6
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

三类「需要用户决策」的 Agent 事件（记忆提案确认 / 自进化提案审阅 / 任务卡产物绑定）
原走挂件弹窗：一次一批、新盖旧、重启即丢。统一改为 notifications 表持久化消息——
按条独立、排队呈现、可补操作、重启不丢；设置页面板与通知页双入口操作后消息状态
双向同步。

## 出界（显式不做）

- created_at 存 TEXT（毫秒字符串）保持实现现状（2286 年前字典序等价数值序；
  与库内其他表 INTEGER 口径不一，留观察）
- 通知推送（系统级 toast 走既有 tauri-plugin-notification 链路，与本批次无关）
