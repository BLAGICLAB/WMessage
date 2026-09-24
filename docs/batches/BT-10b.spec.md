# BT-10b spec — 调度器韧性：tick panic 兜底 + 并发上限（C5-BT-10 拆批 2/2）

## 背景核（finding 三指控逐条对现状）

bot_scheduler.rs:468 finding 三条：
1. **无 panic recovery** —— 属实：循环体 `find_due_tasks(&app).await` 直接在
   spawn 的调度任务里，任何 panic（db_load/audit/broadcast）杀死整个调度器，
   到点任务静默不再执行直到重启。per-task spawn 注释自称隔离 panic，但循环
   本体无保护。
2. **无并发上限** —— 属实：每 tick 对全部到点卡 spawn-and-forget，burst 时
   并发执行无界。
3. **无关闭信号** —— 属实但**按设计接受**：调度器生命周期 = App 生命周期，
   Tauri 退出时 runtime 整体回收；不为它加 shutdown 通道（YAGNI，登记理由）。

设施现成：`futures` crate 在 Cargo.toml（:89）；`AssertUnwindSafe + catch_unwind`
是既有模式（middleware.rs:87/:149、api_handlers/commands.rs:223）。

## 目标 finding（1 条实修，三指控两修一登记）

- **bot_scheduler.rs:468**：
  - 抽 `scheduler_tick(app, sem)`（单轮扫描+分发），循环体改
    `AssertUnwindSafe(scheduler_tick(..)).catch_unwind().await`——tick 内任何
    panic 落 `sched_tick_panic` 审计（panic payload 字符串化截 200）后循环继续。
  - 新增 `SCHED_MAX_CONCURRENT = 4` + `tokio::sync::Semaphore`：per-task spawn
    内 acquire permit 再执行——burst 到点任务排队而非并发打爆 runtime。
  - 关闭信号不加：登记 rationale（生命周期=App，YAGNI）。

## 行为变更

- 有（韧性语义）：tick panic 从「调度器静默死亡」变「记 sched_tick_panic 审计
  后继续下轮」；同 tick >4 张到点卡从「全并发」变「4 并发其余排队」。
  单卡执行/超时/审计语义不变。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot_scheduler.rs`（单文件）。✓
2. budget → scheduler_tick 抽出 + catch_unwind ≈+30/-18；semaphore ≈+8；
   合计 ≈+38/-18 → budget +50/-25。✓
3. findings fix 字段列 ripple → 无（start_scheduler 签名不变，main 调用点不动）。✓

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/<batch-id>/,不主动读回

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BT-10b",
  "family": "scheduler-resilience",
  "expected_files": [
    "src-tauri/src/bot_scheduler.rs"
  ],
  "max_lines_added": 50,
  "max_lines_removed": 25,
  "findings": [
    {"id": "C5-BT-10.4", "file": "src-tauri/src/bot_scheduler.rs", "line": 468, "fix": "scheduler_tick 抽出 + AssertUnwindSafe/catch_unwind tick panic 兜底（sched_tick_panic 审计后续跑）；Semaphore(4) 并发上限；关闭信号按 App 生命周期登记不加。ripple：无（start_scheduler 签名不变）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_scheduler.rs": 46
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  }
}
```
