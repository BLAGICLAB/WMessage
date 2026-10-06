# Batch Spec: T1F-FLAGRACE 测试基建——跨二进制 bot flag 并行竞态修复

```json
{
  "batch_id": "T1F-FLAGRACE",
  "family": "test-fix",
  "expected_files": [
    "docs/batches/T1F-FLAGRACE.spec.md",
    "DEVLOG.md",
    "src-tauri/tests/exec_trace.rs",
    "src-tauri/tests/task_chat_exec.rs"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 10,
  "findings": [
    {"id": "T1F-3", "file": "src-tauri/tests/exec_trace.rs", "line": 92, "fix": "cleanup 不删 bot-enabled.flag：nextest 下 exec_trace 与 task_chat_exec 并行进程共享 target/debug/deps/runtime/flags/，一方删除使对方 run_task_in_chat 撞 BotDisabled（推送门禁实锤两用例齐挂）；setup 幂等重写，flag 常驻无害"},
    {"id": "T1F-4", "file": "src-tauri/tests/task_chat_exec.rs", "line": 81, "fix": "同上：cleanup 与失败路径测试两处删除均移除，仅保留 setup 幂等写入"}
  ]
}
```
