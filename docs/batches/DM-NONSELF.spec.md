# Batch Spec: DM-NONSELF 数据管理——删除非本人的任务卡（软删进回收站）

```json
{
  "batch_id": "DM-NONSELF",
  "family": "data-management",
  "expected_files": [
    "docs/batches/DM-NONSELF.spec.md",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/lib.rs",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 240,
  "max_lines_removed": 20,
  "findings": [
    {"id": "DM-1", "file": "src-tauri/src/db/tasks.rs", "line": 656, "fix": "tasks_delete_non_self：owner_id 非本人且非 NULL 的活跃卡软删进回收站（NULL=本人，图谱设计 §1.1）；dry_run 预检；锁内 RMW 打新基线；广播 tasks-updated(source=api) 防回写循环 + tasks-changed"},
    {"id": "DM-2", "file": "src-tauri/src/db/tasks.rs", "line": 707, "fix": "delete_non_self_locked 纯 DB 段单测锚点：筛选口径（本人/NULL/已回收站不动）+ dry_run 不写 + 空转"}
  ]
}
```
