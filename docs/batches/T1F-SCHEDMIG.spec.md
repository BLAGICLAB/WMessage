# Batch Spec: T1F-SCHEDMIG t1_db_roundtrip 红测修复——对齐定时单源契约

```json
{
  "batch_id": "T1F-SCHEDMIG",
  "family": "test-fix",
  "expected_files": [
    "docs/batches/T1F-SCHEDMIG.spec.md",
    "DEVLOG.md",
    "src-tauri/tests/llm_integration.rs"
  ],
  "max_lines_added": 130,
  "max_lines_removed": 20,
  "findings": [
    {"id": "T1F-1", "file": "src-tauri/tests/llm_integration.rs", "line": 1815, "fix": "schedule 断言对齐 T1 单源契约：tasks.schedule 被迁移清空（None）+ scheduled_jobs 出现 job-<task_id> 行（schedule/content 迁移保真）；RMW 段断言不复活任务卡 schedule、不动 scheduled_jobs 单源"},
    {"id": "T1F-2", "file": "src-tauri/tests/llm_integration.rs", "line": 1707, "fix": "t1_cleanup 补 scheduled_jobs / scheduled_job_runs 清理（迁移产物跨运行残留）"}
  ]
}
```

## 说明

- T1 批入库时带出的存量红测（schedule 回读 None）——根因是测试断言停留在单源迁移前的
  迭代行为：`migrate_legacy_task_schedules` 在每次 open_db 把非空 tasks.schedule 迁往
  `scheduled_jobs` 并清空任务卡侧，测试第二次 open_db 即触发迁移。
- 修复仅测试侧；产品零改动。详细根因见 DEVLOG 同日 T1F 条目。
