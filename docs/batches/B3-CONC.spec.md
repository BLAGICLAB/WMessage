# Batch Spec: B3-CONC

## 目的

按 `docs/AUDIT-FIX-PLAN-2026-09-29.md` B3 批（后端并发与流式稳健）。
B3-5（run_model_loop_core 拆分 749 行大件）按计划「可单列后批」登记延后。

- **B3-1 排队可取消 + 状态口径**：子 agent 排队窗口（global 3/per-session 2
  满）此前整段把行写成 Running——用户看到"运行中"实为排队中，且排队不可取消。
  改造：`SubagentGate::wait_slot_cancellable`（每轮 50ms 退避同时查行终态，
  取消即退队，WaitingGuard/ExecGuard RAII 释放）；Running 写库挪到**拿到槽位并
  复核后**（cancel-vs-runner 竞态闸保留：槽后复核终态即中止，DB 读失败保守中止）。
- **B3-2 墙钟超时先 stop 后收尾**：timeout 掐掉 run future 时，脱离 runtime 的
  在飞 Python/MCP（spawn_blocking 不随 future 取消）此前无人通知——现在先
  `stop.force_stop()` + 750ms grace 让它们自行退出清理，不留孤儿进程。
- **B3-3 流式超时改型 + Client 复用**：① `shared_llm_client()`（OnceLock，
  connect 15s，无默认总超时）三处共用（model_loop/摘要/Planner）——每轮新建
  Client 各建连接池+TLS 握手且 drop 粗暴断连；② model_loop 去掉 300s **总**超时
  （合法长生成会被误杀），改逐 chunk idle 超时 120s（`STREAM_CHUNK_IDLE_TIMEOUT`，
  超时记 `llm.stream_idle_timeout` audit）；③ 摘要/Planner 的 60s 预算改
  per-request `.timeout(60s)` 保留。
- **B3-4 Python 闸有界并发**：`PY_RUN_GATE` std Mutex 全程持锁把所有 Python
  任务串行化，一个长任务堵死整个队列。改 Condvar 许可池（`PY_RUN_MAX_CONCURRENT=2`，
  `PyGateGuard` RAII 归还），EXITING 复查平移进闸门等待循环（等待前/唤醒后/拿到
  后都查，排队者过闸即拒）；doc 生成流持一个许可跨 dotnet+python 兜底
  （内部 run_python_ungated 不重复计闸）。

## 红线

- 排队 FIFO 降级为 best-effort 的既有勘误不变（纯试占 + 50ms 退避）。
- 摘要/Planner 单次请求预算不变（60s）；流式 idle 120s 只判死连接不限总时长。
- 用户可见行为：排队中的子卡状态从"运行中"变为"排队"语义（check 的
  queue_position 已有展示，Running 口径更真实）。

## 测试

- 改造 2 个：`py_run_gate_caps_concurrent_runs`（10 线程抢许可，MAX ≤ 2 且 ≥1）、
  `exiting_check_is_inside_gate_lock`（源码锁：闸门等待循环内必须复查 EXITING）。
- 回归：py 64 / task_chat_exec 14 / llm_integration 37 / model_loop 44 /
  evolution 283 / memory 56 全绿；cargo check 0 error；cargo fmt 过。

## spec 起草后自查三条

1. expected_files 9：七个源文件 + DEVLOG + 本 spec。
2. budget：修改 +240/-65（runtime 重写闸门 + orchestrator 重排）。
3. assertions_min 按 gate 正则填实测-1（staged 全文计数）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "B3-CONC",
  "family": "backend-concurrency",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/B3-CONC.spec.md",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot_plan.rs",
    "src-tauri/src/bot_py.rs",
    "src-tauri/src/py/document.rs",
    "src-tauri/src/py/runtime.rs"
  ],
  "max_lines_added": 450,
  "max_lines_removed": 110,
  "findings": [
    {"id": "B3-1", "file": "src-tauri/src/bot_orchestrator.rs", "line": 806, "fix": "wait_slot_cancellable：排队可取消（500ms 节流查终态）+ Running 挪到槽后写 + 写失败响亮中止"},
    {"id": "B3-2", "file": "src-tauri/src/bot_orchestrator.rs", "line": 1290, "fix": "墙钟触达先 force_stop + 750ms grace 再收尾"},
    {"id": "B3-3", "file": "src-tauri/src/bot_model_loop.rs", "line": 385, "fix": "shared_llm_client + 300s 总超时改 120s 逐 chunk idle + send() 60s 响应头超时（评审 HIGH①）"},
    {"id": "B3-3", "file": "src-tauri/src/bot_chat.rs", "line": 1088, "fix": "摘要走共享客户端 + per-request 60s"},
    {"id": "B3-3", "file": "src-tauri/src/bot_plan.rs", "line": 114, "fix": "Planner 走共享客户端 + per-request 60s"},
    {"id": "B3-4", "file": "src-tauri/src/py/runtime.rs", "line": 300, "fix": "PY_RUN_GATE Mutex → Condvar 许可池（2）+ EXITING 三点复查（等待前/唤醒后/拿到许可后终检）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_orchestrator.rs": 86,
    "src-tauri/src/bot_model_loop.rs": 122,
    "src-tauri/src/bot_chat.rs": 68,
    "src-tauri/src/bot_plan.rs": 20,
    "src-tauri/src/bot_py.rs": 160
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 900,
    "expected_max_comments": 25
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
cargo test py && cargo test --test task_chat_exec && cargo test --test llm_integration
cargo test model_loop && cargo test evolution:: && cargo test memory
```
